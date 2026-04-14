/// Remote CTAP service: orchestrates the virtual FIDO device, CTAPHID framing,
/// and message forwarding between the browser and the RustDesk client.
///
/// Runs on the remote (server) machine. Creates a virtual FIDO device that the
/// browser discovers as a CTAP2 authenticator, then tunnels CTAP2 commands to
/// the client where the user's physical security key is connected.
use crate::server::ctap_virtual_device::{self, VirtualFidoDevice};
use ctap_common::ctap_hid::*;
use hbb_common::{
    anyhow, log,
    message_proto::*,
    tokio::{
        self,
        sync::{mpsc, oneshot},
        time::{self, Duration, Instant},
    },
    ResultType,
};

/// Configuration for the CTAP service.
pub struct CtapServiceConfig {
    /// Maximum time to wait for a response from the client (default: 25 seconds).
    pub client_timeout: Duration,
    /// KEEPALIVE interval (default: 100ms).
    pub keepalive_interval: Duration,
}

impl Default for CtapServiceConfig {
    fn default() -> Self {
        Self {
            client_timeout: Duration::from_secs(25),
            keepalive_interval: Duration::from_millis(100),
        }
    }
}

/// Run the CTAP passthrough service on the remote (server) side.
///
/// Creates a virtual FIDO device, enters the main event loop, and destroys
/// the device on exit.
pub async fn run_ctap_service(
    tx_to_peer: mpsc::UnboundedSender<CtapFrame>,
    mut rx_from_peer: mpsc::UnboundedReceiver<CtapFrame>,
    mut shutdown: oneshot::Receiver<()>,
    config: CtapServiceConfig,
) -> ResultType<()> {
    let device = ctap_virtual_device::create_virtual_fido_device()?;
    log::info!("CTAP service started, virtual FIDO device created");

    let mut assembler = CtapHidAssembler::new();
    let mut next_cid: u32 = 1;
    let mut active_cid: Option<u32> = None;

    let result = run_event_loop(
        device.as_ref(),
        &tx_to_peer,
        &mut rx_from_peer,
        &mut shutdown,
        &mut assembler,
        &mut next_cid,
        &mut active_cid,
        &config,
    )
    .await;

    if let Err(e) = device.destroy() {
        log::warn!("Failed to destroy virtual FIDO device: {}", e);
    }
    log::info!("CTAP service stopped");
    result
}

async fn run_event_loop(
    device: &dyn VirtualFidoDevice,
    tx_to_peer: &mpsc::UnboundedSender<CtapFrame>,
    rx_from_peer: &mut mpsc::UnboundedReceiver<CtapFrame>,
    shutdown: &mut oneshot::Receiver<()>,
    assembler: &mut CtapHidAssembler,
    next_cid: &mut u32,
    active_cid: &mut Option<u32>,
    config: &CtapServiceConfig,
) -> ResultType<()> {
    let mut keepalive_interval = time::interval(config.keepalive_interval);
    keepalive_interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    let mut client_deadline: Option<Instant> = None;

    loop {
        // The uhid read is blocking. We use spawn_blocking with a transmuted
        // fat pointer to move the trait object reference into the blocking thread.
        let report_result = tokio::task::spawn_blocking({
            let device_raw: [usize; 2] = unsafe {
                std::mem::transmute(device as *const dyn VirtualFidoDevice)
            };
            move || {
                let device: &dyn VirtualFidoDevice = unsafe {
                    std::mem::transmute::<[usize; 2], *const dyn VirtualFidoDevice>(device_raw)
                        .as_ref()
                        .unwrap()
                };
                device.read_output_report()
            }
        });

        tokio::select! {
            _ = &mut *shutdown => {
                log::info!("CTAP service received shutdown signal");
                return Ok(());
            }

            result = report_result => {
                let report = result??;
                handle_uhid_report(
                    &report, device, tx_to_peer, assembler,
                    next_cid, active_cid, &mut client_deadline, config,
                )?;
            }

            Some(frame) = rx_from_peer.recv(), if active_cid.is_some() => {
                handle_client_response(
                    frame, device, active_cid, assembler,
                )?;
                client_deadline = None;
            }

            _ = keepalive_interval.tick(), if active_cid.is_some() => {
                if let Some(cid) = *active_cid {
                    if let Some(deadline) = client_deadline {
                        if Instant::now() >= deadline {
                            log::warn!("CTAP client response timeout");
                            send_error(device, cid, ERR_MSG_TIMEOUT)?;
                            *active_cid = None;
                            client_deadline = None;
                            assembler.reset();
                            continue;
                        }
                    }
                    let ka = CtapHidMessage::keepalive(cid, STATUS_UPNEEDED);
                    send_ctaphid(device, &ka)?;
                }
            }
        }
    }
}

fn handle_uhid_report(
    report: &[u8; 64],
    device: &dyn VirtualFidoDevice,
    tx_to_peer: &mpsc::UnboundedSender<CtapFrame>,
    assembler: &mut CtapHidAssembler,
    next_cid: &mut u32,
    active_cid: &mut Option<u32>,
    client_deadline: &mut Option<Instant>,
    config: &CtapServiceConfig,
) -> ResultType<()> {
    let msg = match assembler.feed(report) {
        Ok(Some(msg)) => msg,
        Ok(None) => return Ok(()),
        Err(e) => {
            log::warn!("CTAPHID framing error: {}", e);
            let cid = u32::from_be_bytes([report[0], report[1], report[2], report[3]]);
            send_error(device, cid, ERR_INVALID_SEQ)?;
            assembler.reset();
            return Ok(());
        }
    };

    match msg.cmd {
        CTAPHID_INIT => {
            handle_init(device, &msg, next_cid)?;
        }
        CTAPHID_PING => {
            let resp = CtapHidMessage::ping_response(msg.cid, msg.payload.clone());
            send_ctaphid(device, &resp)?;
        }
        CTAPHID_CBOR => {
            if active_cid.is_some() {
                send_error(device, msg.cid, ERR_CHANNEL_BUSY)?;
                return Ok(());
            }

            // Security filter: only allow safe CTAP2 commands
            if !is_allowed_ctap2_command(&msg.payload) {
                let cmd_byte = msg.payload.first().copied().unwrap_or(0xFF);
                log::warn!(
                    "Blocked disallowed CTAP2 command 0x{:02x} from remote",
                    cmd_byte
                );
                send_error(device, msg.cid, ERR_INVALID_CMD)?;
                return Ok(());
            }

            *active_cid = Some(msg.cid);
            *client_deadline = Some(Instant::now() + config.client_timeout);

            let frame = CtapFrame {
                command: CTAPHID_CBOR as u32,
                payload: msg.payload.into(),
                is_response: false,
                error_code: 0,
                ..Default::default()
            };
            tx_to_peer.send(frame)?;

            log::debug!(
                "Forwarded CTAP CBOR request to client, CID={:#010x}",
                msg.cid
            );
        }
        CTAPHID_CANCEL => {
            if let Some(cid) = *active_cid {
                if msg.cid == cid {
                    let frame = CtapFrame {
                        command: CTAPHID_CANCEL as u32,
                        payload: vec![].into(),
                        is_response: false,
                        error_code: 0,
                        ..Default::default()
                    };
                    tx_to_peer.send(frame)?;
                    *active_cid = None;
                    *client_deadline = None;
                    log::debug!("Forwarded CTAP CANCEL to client");
                }
            }
        }
        CTAPHID_MSG => {
            send_error(device, msg.cid, ERR_INVALID_CMD)?;
        }
        _ => {
            log::debug!("Unsupported CTAPHID command: 0x{:02x}", msg.cmd);
            send_error(device, msg.cid, ERR_INVALID_CMD)?;
        }
    }

    Ok(())
}

fn handle_init(
    device: &dyn VirtualFidoDevice,
    msg: &CtapHidMessage,
    next_cid: &mut u32,
) -> ResultType<()> {
    if msg.payload.len() < 8 {
        send_error(device, msg.cid, ERR_INVALID_LEN)?;
        return Ok(());
    }

    let mut nonce = [0u8; 8];
    nonce.copy_from_slice(&msg.payload[..8]);

    let allocated_cid = *next_cid;
    *next_cid += 1;
    if *next_cid == 0 || *next_cid == BROADCAST_CID {
        *next_cid = 1;
    }

    let resp = CtapHidMessage::init_response(&nonce, allocated_cid, 0x04);
    send_ctaphid(device, &resp)?;

    log::debug!("Allocated CTAPHID channel CID={:#010x}", allocated_cid);
    Ok(())
}

fn handle_client_response(
    frame: CtapFrame,
    device: &dyn VirtualFidoDevice,
    active_cid: &mut Option<u32>,
    assembler: &mut CtapHidAssembler,
) -> ResultType<()> {
    let cid = match *active_cid {
        Some(cid) => cid,
        None => {
            log::warn!("Received CTAP response but no active transaction");
            return Ok(());
        }
    };

    if frame.error_code != 0 {
        let error_byte = (frame.error_code & 0xFF) as u8;
        let resp = CtapHidMessage {
            cid,
            cmd: CTAPHID_CBOR,
            payload: vec![error_byte],
        };
        send_ctaphid(device, &resp)?;
    } else {
        let resp = CtapHidMessage {
            cid,
            cmd: frame.command as u8,
            payload: frame.payload.to_vec(),
        };
        send_ctaphid(device, &resp)?;
    }

    *active_cid = None;
    assembler.reset();
    log::debug!("Forwarded CTAP response to browser, CID={:#010x}", cid);
    Ok(())
}

fn send_ctaphid(device: &dyn VirtualFidoDevice, msg: &CtapHidMessage) -> ResultType<()> {
    let packets = fragment(msg).map_err(|e| anyhow::anyhow!("{}", e))?;
    for pkt in &packets {
        device.write_input_report(pkt)?;
    }
    Ok(())
}

fn send_error(device: &dyn VirtualFidoDevice, cid: u32, code: u8) -> ResultType<()> {
    let msg = CtapHidMessage::error(cid, code);
    send_ctaphid(device, &msg)
}
