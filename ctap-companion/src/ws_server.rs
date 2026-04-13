/// WebSocket server for the CTAP Companion App.
///
/// Listens on localhost, validates origins, accepts one session at a time,
/// and relays CTAP commands to physical FIDO keys via ctap-common.
use crate::protocol::{ClientMessage, ServerMessage};
use base64::Engine;
use ctap_common::ctap_hid::{is_allowed_ctap2_command, CTAPHID_CBOR};
use ctap_common::fido_relay::{CtapRelayResult, LocalAuthenticator};
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

const _IDLE_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes — reserved for future idle shutdown

pub struct WsServer {
    port: u16,
    allowed_origins: Vec<String>,
}

impl WsServer {
    pub fn new(port: u16, additional_origins: Vec<String>) -> Self {
        let mut allowed_origins = vec![
            "https://web.rustdesk.com".to_string(),
            "http://localhost".to_string(),
            "https://localhost".to_string(),
            "http://127.0.0.1".to_string(),
            "https://127.0.0.1".to_string(),
        ];
        allowed_origins.extend(additional_origins);
        Self {
            port,
            allowed_origins,
        }
    }

    pub async fn run(&self, shutdown: Arc<Notify>) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", self.port)).await?;
        log::info!("CTAP Companion listening on 127.0.0.1:{}", self.port);

        let has_active_session = Arc::new(AtomicBool::new(false));

        loop {
            tokio::select! {
                _ = shutdown.notified() => {
                    log::info!("Shutdown signal received");
                    return Ok(());
                }
                result = listener.accept() => {
                    let (stream, addr) = result?;
                    log::info!("Connection from {}", addr);

                    if has_active_session.load(Ordering::SeqCst) {
                        log::warn!("Rejecting connection: already have an active session");
                        drop(stream);
                        continue;
                    }

                    let active = has_active_session.clone();
                    let origins = self.allowed_origins.clone();
                    tokio::spawn(async move {
                        active.store(true, Ordering::SeqCst);
                        if let Err(e) = handle_connection(stream, &origins).await {
                            log::error!("Connection error: {}", e);
                        }
                        active.store(false, Ordering::SeqCst);
                    });
                }
            }
        }
    }
}

async fn handle_connection(
    stream: TcpStream,
    allowed_origins: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let origins = allowed_origins.to_vec();
    let ws_stream = tokio_tungstenite::accept_hdr_async(stream, |req: &Request, resp: Response| {
        // Validate Origin header
        if let Some(origin) = req.headers().get("origin") {
            let origin_str = origin.to_str().unwrap_or("");
            if !origins.iter().any(|o| origin_str.starts_with(o)) {
                log::warn!("Rejected connection from origin: {}", origin_str);
                return Err(Response::builder()
                    .status(403)
                    .body(None)
                    .unwrap());
            }
        }
        Ok(resp)
    })
    .await?;

    let (mut ws_tx, mut ws_rx) = ws_stream.split();
    #[allow(unused_assignments)]
    let mut cancel_tx: Option<std::sync::mpsc::Sender<()>> = None;

    while let Some(msg) = ws_rx.next().await {
        let msg = msg?;
        if msg.is_close() {
            break;
        }
        if !msg.is_text() {
            continue;
        }

        let text = msg.to_text()?;
        let client_msg: ClientMessage = match serde_json::from_str(text) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("Invalid JSON from client: {}", e);
                continue;
            }
        };

        match client_msg {
            ClientMessage::Hello { version, .. } => {
                let fido_available = LocalAuthenticator::is_available();
                let response = ServerMessage::HelloAck {
                    version: version.min(1),
                    fido_available,
                    device_name: String::new(), // Could enumerate from hidapi
                };
                let json = serde_json::to_string(&response)?;
                ws_tx.send(Message::Text(json.into())).await?;
            }
            ClientMessage::CtapRelay {
                id,
                command,
                payload,
            } => {
                let payload_bytes = base64::engine::general_purpose::STANDARD.decode(&payload)?;

                // Security: validate allowed CTAP2 commands
                if command == CTAPHID_CBOR as u32 && !is_allowed_ctap2_command(&payload_bytes) {
                    let cmd_byte = payload_bytes.first().copied().unwrap_or(0xFF);
                    log::warn!("Blocked disallowed CTAP2 command 0x{:02x}", cmd_byte);
                    let response = ServerMessage::CtapResponse {
                        id,
                        command,
                        payload: String::new(),
                        error_code: 0x01,
                    };
                    let json = serde_json::to_string(&response)?;
                    ws_tx.send(Message::Text(json.into())).await?;
                    continue;
                }

                let (ctx, crx) = std::sync::mpsc::channel();
                cancel_tx = Some(ctx);

                // Relay in a blocking thread
                let result = tokio::task::spawn_blocking(move || {
                    let auth = match LocalAuthenticator::open_first() {
                        Ok(a) => a,
                        Err(e) => {
                            log::error!("No FIDO authenticator: {}", e);
                            return CtapRelayResult::Error(0x2E);
                        }
                    };
                    auth.relay_command(&payload_bytes, Duration::from_secs(25), &crx)
                })
                .await?;

                cancel_tx = None;

                let (resp_payload, error_code) = match result {
                    CtapRelayResult::Response(data) => {
                        (base64::engine::general_purpose::STANDARD.encode(&data), 0)
                    }
                    CtapRelayResult::Error(code) => (String::new(), code),
                };

                let response = ServerMessage::CtapResponse {
                    id,
                    command,
                    payload: resp_payload,
                    error_code,
                };
                let json = serde_json::to_string(&response)?;
                ws_tx.send(Message::Text(json.into())).await?;
            }
            ClientMessage::CtapCancel { id } => {
                if let Some(tx) = cancel_tx.take() {
                    let _ = tx.send(());
                }
                let response = ServerMessage::CtapResponse {
                    id,
                    command: CTAPHID_CBOR as u32,
                    payload: String::new(),
                    error_code: 0x2D, // CTAP2_ERR_KEEPALIVE_CANCEL
                };
                let json = serde_json::to_string(&response)?;
                ws_tx.send(Message::Text(json.into())).await?;
            }
        }
    }

    log::info!("WebSocket session ended");
    Ok(())
}
