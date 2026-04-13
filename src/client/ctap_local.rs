/// Local authenticator driver for the native RustDesk client.
///
/// Receives CTAP2 CBOR commands from the remote service (via CtapFrame),
/// relays them to a physical FIDO2 security key via hidapi, and returns
/// the response. Uses ctap-common for CTAPHID framing and HID I/O.
use ctap_common::ctap_hid::CTAPHID_CBOR;
use ctap_common::fido_relay::{CtapRelayResult, LocalAuthenticator};
use hbb_common::message_proto::CtapFrame;
use std::time::Duration;

/// Async wrapper that spawns a blocking thread for HID I/O.
///
/// Opens the physical authenticator, relays the CTAP2 command, and returns
/// a CtapFrame ready to send back to the remote service.
pub async fn relay_to_physical_key(
    cbor_payload: Vec<u8>,
    cancel_rx: std::sync::mpsc::Receiver<()>,
) -> CtapFrame {
    let result = tokio::task::spawn_blocking(move || {
        let auth = match LocalAuthenticator::open_first() {
            Ok(a) => a,
            Err(e) => {
                log::error!("No FIDO authenticator available: {}", e);
                return CtapRelayResult::Error(0x2E); // CTAP2_ERR_NO_CREDENTIALS
            }
        };

        auth.relay_command(&cbor_payload, Duration::from_secs(25), &cancel_rx)
    })
    .await;

    match result {
        Ok(CtapRelayResult::Response(payload)) => CtapFrame {
            command: CTAPHID_CBOR as u32,
            payload: payload.into(),
            is_response: true,
            error_code: 0,
            ..Default::default()
        },
        Ok(CtapRelayResult::Error(code)) => CtapFrame {
            command: CTAPHID_CBOR as u32,
            payload: vec![].into(),
            is_response: true,
            error_code: code,
            ..Default::default()
        },
        Err(e) => {
            log::error!("CTAP relay task panicked: {}", e);
            CtapFrame {
                command: CTAPHID_CBOR as u32,
                payload: vec![].into(),
                is_response: true,
                error_code: 0x01, // CTAP1_ERR_OTHER
                ..Default::default()
            }
        }
    }
}

/// Check if a local FIDO authenticator is available.
pub fn is_local_fido_available() -> bool {
    LocalAuthenticator::is_available()
}
