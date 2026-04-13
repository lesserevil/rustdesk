/// FIDO2 relay logic: communicates with a physical security key via hidapi.
///
/// Shared between the native RustDesk client (`src/client/ctap_local.rs`)
/// and the standalone CTAP Companion App (`ctap-companion/`).
use crate::ctap_hid::*;
use std::time::Duration;

const FIDO_USAGE_PAGE: u16 = 0xF1D0;
const FIDO_USAGE: u16 = 0x01;

/// Result of attempting to relay a CTAP command to the physical authenticator.
#[derive(Debug)]
pub enum CtapRelayResult {
    /// Successful response from the authenticator.
    Response(Vec<u8>),
    /// Error from the authenticator or driver (CTAP2 error code).
    Error(u32),
}

/// Handle to a physical FIDO2 authenticator connected via USB.
pub struct LocalAuthenticator {
    device: hidapi::HidDevice,
}

impl LocalAuthenticator {
    /// Find and open the first available FIDO2 authenticator.
    ///
    /// Enumerates USB HID devices, filters for FIDO Alliance usage page (0xF1D0),
    /// and opens the first matching device.
    pub fn open_first() -> Result<Self, Box<dyn std::error::Error>> {
        let api = hidapi::HidApi::new()?;

        let device_info = api
            .device_list()
            .find(|d| d.usage_page() == FIDO_USAGE_PAGE && d.usage() == FIDO_USAGE)
            .ok_or_else(|| {
                "No FIDO2 authenticator found. Is a security key connected?"
            })?;

        log::info!(
            "Found FIDO device: {} (VID={:04x}, PID={:04x})",
            device_info.product_string().unwrap_or("Unknown"),
            device_info.vendor_id(),
            device_info.product_id(),
        );

        let device = device_info.open_device(&api)?;
        Ok(Self { device })
    }

    /// Check if a physical FIDO authenticator is available without opening it.
    pub fn is_available() -> bool {
        match hidapi::HidApi::new() {
            Ok(api) => api
                .device_list()
                .any(|d| d.usage_page() == FIDO_USAGE_PAGE && d.usage() == FIDO_USAGE),
            Err(_) => false,
        }
    }

    /// Send a CTAP2 CBOR command and wait for the response.
    ///
    /// This is a BLOCKING function. Call from `tokio::task::spawn_blocking`.
    pub fn relay_command(
        &self,
        cbor_payload: &[u8],
        timeout: Duration,
        cancel: &std::sync::mpsc::Receiver<()>,
    ) -> CtapRelayResult {
        // Step 1: Allocate channel
        let cid = match self.allocate_channel() {
            Ok(cid) => cid,
            Err(e) => {
                log::error!("CTAPHID INIT failed: {}", e);
                return CtapRelayResult::Error(0x01); // CTAP1_ERR_OTHER
            }
        };

        // Step 2: Send CBOR command
        let msg = CtapHidMessage {
            cid,
            cmd: CTAPHID_CBOR,
            payload: cbor_payload.to_vec(),
        };
        if let Err(e) = self.send_message(&msg) {
            log::error!("Failed to send CTAP command: {}", e);
            return CtapRelayResult::Error(0x01);
        }

        // Step 3: Wait for response, handling KEEPALIVEs
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if cancel.try_recv().is_ok() {
                log::info!("CTAP operation cancelled by user");
                let cancel_msg = CtapHidMessage {
                    cid,
                    cmd: CTAPHID_CANCEL,
                    payload: vec![],
                };
                let _ = self.send_message(&cancel_msg);
                return CtapRelayResult::Error(0x2D); // CTAP2_ERR_KEEPALIVE_CANCEL
            }

            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                log::warn!("CTAP operation timed out");
                return CtapRelayResult::Error(0x2D);
            }

            let read_timeout = remaining.min(Duration::from_millis(100));
            match self.read_message(cid, read_timeout) {
                Ok(Some(resp)) => {
                    if resp.cmd == CTAPHID_KEEPALIVE {
                        log::trace!("Received KEEPALIVE from physical key");
                        continue;
                    }
                    if resp.cmd == CTAPHID_CBOR {
                        return CtapRelayResult::Response(resp.payload);
                    }
                    if resp.cmd == CTAPHID_ERROR {
                        let code = resp.payload.first().copied().unwrap_or(0x7F);
                        return CtapRelayResult::Error(code as u32);
                    }
                    log::warn!("Unexpected CTAPHID response: cmd=0x{:02x}", resp.cmd);
                    continue;
                }
                Ok(None) => continue, // timeout on this read, loop back
                Err(e) => {
                    log::error!("Error reading from authenticator: {}", e);
                    return CtapRelayResult::Error(0x01);
                }
            }
        }
    }

    fn allocate_channel(&self) -> Result<u32, Box<dyn std::error::Error>> {
        let mut nonce = [0u8; 8];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut nonce);

        let init_msg = CtapHidMessage {
            cid: BROADCAST_CID,
            cmd: CTAPHID_INIT,
            payload: nonce.to_vec(),
        };
        self.send_message(&init_msg)?;

        match self.read_message(BROADCAST_CID, Duration::from_secs(1))? {
            Some(resp) => {
                if resp.cmd != CTAPHID_INIT || resp.payload.len() < 17 {
                    return Err("Invalid INIT response".into());
                }
                if resp.payload[..8] != nonce {
                    return Err("INIT nonce mismatch".into());
                }
                let cid = u32::from_be_bytes([
                    resp.payload[8],
                    resp.payload[9],
                    resp.payload[10],
                    resp.payload[11],
                ]);
                log::debug!("Allocated CID {:#010x} from physical key", cid);
                Ok(cid)
            }
            None => Err("INIT response timeout".into()),
        }
    }

    fn send_message(&self, msg: &CtapHidMessage) -> Result<(), Box<dyn std::error::Error>> {
        let packets = fragment(msg)?;
        for pkt in &packets {
            // hidapi write: first byte is report ID (0x00 for FIDO with no report ID)
            let mut buf = [0u8; 65];
            buf[0] = 0x00;
            buf[1..65].copy_from_slice(pkt);
            self.device.write(&buf)?;
        }
        Ok(())
    }

    fn read_message(
        &self,
        expected_cid: u32,
        timeout: Duration,
    ) -> Result<Option<CtapHidMessage>, Box<dyn std::error::Error>> {
        let mut assembler = CtapHidAssembler::new();
        let deadline = std::time::Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }

            let timeout_ms = remaining.as_millis().min(i32::MAX as u128) as i32;
            let mut buf = [0u8; HID_REPORT_SIZE];
            let n = self.device.read_timeout(&mut buf, timeout_ms)?;

            if n == 0 {
                return Ok(None);
            }
            if n != HID_REPORT_SIZE {
                log::warn!("Short HID read: {} bytes", n);
                continue;
            }

            match assembler.feed(&buf) {
                Ok(Some(msg)) => {
                    if expected_cid != BROADCAST_CID && msg.cid != expected_cid {
                        log::warn!(
                            "CID mismatch: expected {:#010x}, got {:#010x}",
                            expected_cid,
                            msg.cid
                        );
                        assembler.reset();
                        continue;
                    }
                    return Ok(Some(msg));
                }
                Ok(None) => continue,
                Err(e) => {
                    log::warn!("CTAPHID framing error from physical key: {}", e);
                    assembler.reset();
                    continue;
                }
            }
        }
    }
}
