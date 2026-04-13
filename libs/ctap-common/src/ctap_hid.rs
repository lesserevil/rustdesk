/// CTAPHID framing protocol implementation.
///
/// Handles assembly/disassembly of CTAPHID messages from 64-byte HID reports,
/// as specified in FIDO CTAP 2.1, Section 11.2 (USB HID).

/// HID report size for FIDO devices (always 64 bytes).
pub const HID_REPORT_SIZE: usize = 64;

/// Max payload in an initialization packet (64 - 4 CID - 1 CMD - 2 LEN = 57).
pub const INIT_PACKET_DATA_SIZE: usize = 57;

/// Max payload in a continuation packet (64 - 4 CID - 1 SEQ = 59).
pub const CONT_PACKET_DATA_SIZE: usize = 59;

/// Maximum number of continuation packets (SEQ 0x00-0x7F = 128).
pub const MAX_CONT_PACKETS: usize = 128;

/// Maximum total payload size: 57 + 128*59 = 7609 bytes.
pub const MAX_PAYLOAD_SIZE: usize = INIT_PACKET_DATA_SIZE + MAX_CONT_PACKETS * CONT_PACKET_DATA_SIZE;

/// Broadcast channel ID (used for CTAPHID_INIT).
pub const BROADCAST_CID: u32 = 0xFFFFFFFF;

// CTAPHID command bytes (without the 0x80 bit -- that's added during framing).
pub const CTAPHID_PING: u8 = 0x01;
pub const CTAPHID_MSG: u8 = 0x03;
pub const CTAPHID_LOCK: u8 = 0x04;
pub const CTAPHID_INIT: u8 = 0x06;
pub const CTAPHID_WINK: u8 = 0x08;
pub const CTAPHID_CBOR: u8 = 0x10;
pub const CTAPHID_CANCEL: u8 = 0x11;
pub const CTAPHID_KEEPALIVE: u8 = 0x3B;
pub const CTAPHID_ERROR: u8 = 0x3F;

// KEEPALIVE status codes.
pub const STATUS_PROCESSING: u8 = 0x01;
pub const STATUS_UPNEEDED: u8 = 0x02;

// Error codes.
pub const ERR_INVALID_CMD: u8 = 0x01;
pub const ERR_INVALID_PAR: u8 = 0x02;
pub const ERR_INVALID_LEN: u8 = 0x03;
pub const ERR_INVALID_SEQ: u8 = 0x04;
pub const ERR_MSG_TIMEOUT: u8 = 0x05;
pub const ERR_CHANNEL_BUSY: u8 = 0x06;
pub const ERR_LOCK_REQUIRED: u8 = 0x0A;
pub const ERR_INVALID_CHANNEL: u8 = 0x0B;
pub const ERR_OTHER: u8 = 0x7F;

/// CTAP2 commands that are allowed through the tunnel.
/// Blocks authenticatorReset (0x07), BioEnrollment (0x09),
/// CredentialManagement (0x0A), and Config (0x0D).
pub const ALLOWED_CTAP2_COMMANDS: &[u8] = &[0x01, 0x02, 0x04, 0x06, 0x08, 0x0B];

pub fn is_allowed_ctap2_command(payload: &[u8]) -> bool {
    payload
        .first()
        .map(|cmd| ALLOWED_CTAP2_COMMANDS.contains(cmd))
        .unwrap_or(false)
}

#[derive(Debug, thiserror::Error)]
pub enum CtapHidError {
    #[error("Expected init packet (bit 7 set), got continuation")]
    ExpectedInitPacket,

    #[error("Invalid continuation sequence: expected {expected}, got {got}")]
    InvalidSequence { expected: u8, got: u8 },

    #[error("CID mismatch: expected {expected:#010x}, got {got:#010x}")]
    CidMismatch { expected: u32, got: u32 },

    #[error("Payload too large: {size} bytes exceeds max {max}")]
    PayloadTooLarge { size: usize, max: usize },

    #[error("Invalid report size: expected 64, got {0}")]
    InvalidReportSize(usize),
}

/// A fully reassembled CTAPHID message.
#[derive(Debug, Clone)]
pub struct CtapHidMessage {
    /// Channel ID (4 bytes, big-endian on wire).
    pub cid: u32,
    /// Command byte (without the 0x80 flag).
    pub cmd: u8,
    /// Complete payload (reassembled from all packets).
    pub payload: Vec<u8>,
}

impl CtapHidMessage {
    /// Create a CTAPHID_INIT response.
    pub fn init_response(nonce: &[u8; 8], new_cid: u32, capabilities: u8) -> Self {
        let mut payload = Vec::with_capacity(17);
        payload.extend_from_slice(nonce);
        payload.extend_from_slice(&new_cid.to_be_bytes());
        payload.push(2); // protocol version
        payload.push(0); // major version
        payload.push(0); // minor version
        payload.push(0); // build version
        payload.push(capabilities);

        Self {
            cid: BROADCAST_CID,
            cmd: CTAPHID_INIT,
            payload,
        }
    }

    /// Create a CTAPHID_KEEPALIVE message.
    pub fn keepalive(cid: u32, status: u8) -> Self {
        Self {
            cid,
            cmd: CTAPHID_KEEPALIVE,
            payload: vec![status],
        }
    }

    /// Create a CTAPHID_ERROR message.
    pub fn error(cid: u32, error_code: u8) -> Self {
        Self {
            cid,
            cmd: CTAPHID_ERROR,
            payload: vec![error_code],
        }
    }

    /// Create a CTAPHID_PING response (echo).
    pub fn ping_response(cid: u32, data: Vec<u8>) -> Self {
        Self {
            cid,
            cmd: CTAPHID_PING,
            payload: data,
        }
    }
}

/// State machine for reassembling multi-packet CTAPHID messages.
pub struct CtapHidAssembler {
    current_cid: Option<u32>,
    current_cmd: u8,
    expected_len: usize,
    buffer: Vec<u8>,
    next_seq: u8,
}

impl CtapHidAssembler {
    pub fn new() -> Self {
        Self {
            current_cid: None,
            current_cmd: 0,
            expected_len: 0,
            buffer: Vec::new(),
            next_seq: 0,
        }
    }

    /// Feed a 64-byte HID report to the assembler.
    ///
    /// Returns `Ok(Some(msg))` when a complete message is assembled,
    /// `Ok(None)` when more packets are needed, or `Err` on framing error.
    /// On error, the assembler resets to idle state.
    pub fn feed(&mut self, report: &[u8; HID_REPORT_SIZE]) -> Result<Option<CtapHidMessage>, CtapHidError> {
        let cid = parse_cid(report);

        if is_init_packet(report) {
            // New init packet -- if we were assembling, the previous message was abandoned.
            let cmd = report[4] & 0x7F;
            let total_len = ((report[5] as usize) << 8) | (report[6] as usize);

            if total_len > MAX_PAYLOAD_SIZE {
                self.reset();
                return Err(CtapHidError::PayloadTooLarge {
                    size: total_len,
                    max: MAX_PAYLOAD_SIZE,
                });
            }

            let data_in_init = total_len.min(INIT_PACKET_DATA_SIZE);
            let mut buffer = Vec::with_capacity(total_len);
            buffer.extend_from_slice(&report[7..7 + data_in_init]);

            if buffer.len() >= total_len {
                // Complete in a single packet.
                buffer.truncate(total_len);
                self.reset();
                return Ok(Some(CtapHidMessage {
                    cid,
                    cmd,
                    payload: buffer,
                }));
            }

            // Need continuation packets.
            self.current_cid = Some(cid);
            self.current_cmd = cmd;
            self.expected_len = total_len;
            self.buffer = buffer;
            self.next_seq = 0;
            return Ok(None);
        }

        // Continuation packet.
        let Some(expected_cid) = self.current_cid else {
            self.reset();
            return Err(CtapHidError::ExpectedInitPacket);
        };

        if cid != expected_cid {
            self.reset();
            return Err(CtapHidError::CidMismatch {
                expected: expected_cid,
                got: cid,
            });
        }

        let seq = report[4];
        if seq != self.next_seq {
            self.reset();
            return Err(CtapHidError::InvalidSequence {
                expected: self.next_seq,
                got: seq,
            });
        }

        let remaining = self.expected_len - self.buffer.len();
        let data_in_cont = remaining.min(CONT_PACKET_DATA_SIZE);
        self.buffer.extend_from_slice(&report[5..5 + data_in_cont]);
        self.next_seq += 1;

        if self.buffer.len() >= self.expected_len {
            self.buffer.truncate(self.expected_len);
            let msg = CtapHidMessage {
                cid: expected_cid,
                cmd: self.current_cmd,
                payload: std::mem::take(&mut self.buffer),
            };
            self.reset();
            return Ok(Some(msg));
        }

        Ok(None)
    }

    /// Reset the assembler to idle state.
    pub fn reset(&mut self) {
        self.current_cid = None;
        self.current_cmd = 0;
        self.expected_len = 0;
        self.buffer.clear();
        self.next_seq = 0;
    }

    /// Returns true if the assembler is currently assembling a multi-packet message.
    pub fn is_busy(&self) -> bool {
        self.current_cid.is_some()
    }
}

fn is_init_packet(report: &[u8; HID_REPORT_SIZE]) -> bool {
    report[4] & 0x80 != 0
}

fn parse_cid(report: &[u8; HID_REPORT_SIZE]) -> u32 {
    u32::from_be_bytes([report[0], report[1], report[2], report[3]])
}

/// Fragment a CTAPHID message into 64-byte HID reports.
pub fn fragment(msg: &CtapHidMessage) -> Result<Vec<[u8; HID_REPORT_SIZE]>, CtapHidError> {
    if msg.payload.len() > MAX_PAYLOAD_SIZE {
        return Err(CtapHidError::PayloadTooLarge {
            size: msg.payload.len(),
            max: MAX_PAYLOAD_SIZE,
        });
    }

    let mut packets = Vec::new();
    let cid_bytes = msg.cid.to_be_bytes();
    let total_len = msg.payload.len();

    // Build init packet.
    let mut init = [0u8; HID_REPORT_SIZE];
    init[0..4].copy_from_slice(&cid_bytes);
    init[4] = msg.cmd | 0x80;
    init[5] = ((total_len >> 8) & 0xFF) as u8;
    init[6] = (total_len & 0xFF) as u8;

    let first_chunk = total_len.min(INIT_PACKET_DATA_SIZE);
    init[7..7 + first_chunk].copy_from_slice(&msg.payload[..first_chunk]);
    packets.push(init);

    // Build continuation packets.
    let mut offset = first_chunk;
    let mut seq: u8 = 0;
    while offset < total_len {
        let mut cont = [0u8; HID_REPORT_SIZE];
        cont[0..4].copy_from_slice(&cid_bytes);
        cont[4] = seq;

        let chunk = (total_len - offset).min(CONT_PACKET_DATA_SIZE);
        cont[5..5 + chunk].copy_from_slice(&msg.payload[offset..offset + chunk]);
        packets.push(cont);

        offset += chunk;
        seq += 1;
    }

    Ok(packets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_packet_cbor() {
        let msg = CtapHidMessage {
            cid: 0x00000001,
            cmd: CTAPHID_CBOR,
            payload: vec![0x04], // authenticatorGetInfo
        };

        let packets = fragment(&msg).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0][0..4], [0x00, 0x00, 0x00, 0x01]);
        assert_eq!(packets[0][4], 0x90); // CBOR | 0x80
        assert_eq!(packets[0][5], 0x00);
        assert_eq!(packets[0][6], 0x01);
        assert_eq!(packets[0][7], 0x04);

        let mut asm = CtapHidAssembler::new();
        let result = asm.feed(&packets[0]).unwrap();
        let reassembled = result.expect("Should be complete in one packet");
        assert_eq!(reassembled.cid, 0x00000001);
        assert_eq!(reassembled.cmd, CTAPHID_CBOR);
        assert_eq!(reassembled.payload, vec![0x04]);
    }

    #[test]
    fn test_multi_packet_message() {
        let payload: Vec<u8> = (0..200).map(|i| (i % 256) as u8).collect();
        let msg = CtapHidMessage {
            cid: 0xDEADBEEF,
            cmd: CTAPHID_CBOR,
            payload: payload.clone(),
        };

        let packets = fragment(&msg).unwrap();
        // 57 in init + ceil((200-57)/59) = ceil(143/59) = 3 continuations = 4 total
        assert_eq!(packets.len(), 4);

        let mut asm = CtapHidAssembler::new();
        assert!(asm.feed(&packets[0]).unwrap().is_none());
        assert!(asm.feed(&packets[1]).unwrap().is_none());
        assert!(asm.feed(&packets[2]).unwrap().is_none());
        let result = asm.feed(&packets[3]).unwrap().unwrap();
        assert_eq!(result.cid, 0xDEADBEEF);
        assert_eq!(result.cmd, CTAPHID_CBOR);
        assert_eq!(result.payload, payload);
    }

    #[test]
    fn test_init_flow() {
        let nonce: [u8; 8] = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        let init_req = CtapHidMessage {
            cid: BROADCAST_CID,
            cmd: CTAPHID_INIT,
            payload: nonce.to_vec(),
        };

        let packets = fragment(&init_req).unwrap();
        assert_eq!(packets.len(), 1);

        let mut asm = CtapHidAssembler::new();
        let req = asm.feed(&packets[0]).unwrap().unwrap();
        assert_eq!(req.cid, BROADCAST_CID);
        assert_eq!(req.cmd, CTAPHID_INIT);
        assert_eq!(req.payload.len(), 8);

        let resp = CtapHidMessage::init_response(&nonce, 0x00000001, 0x04);
        assert_eq!(resp.payload.len(), 17);
        assert_eq!(&resp.payload[0..8], &nonce);
        assert_eq!(&resp.payload[8..12], &[0, 0, 0, 1]);
        assert_eq!(resp.payload[12], 2);
        assert_eq!(resp.payload[16], 0x04);
    }

    #[test]
    fn test_sequence_error() {
        let payload = vec![0u8; 200];
        let msg = CtapHidMessage {
            cid: 0x00000001,
            cmd: CTAPHID_CBOR,
            payload,
        };

        let mut packets = fragment(&msg).unwrap();
        packets.swap(2, 3);

        let mut asm = CtapHidAssembler::new();
        assert!(asm.feed(&packets[0]).unwrap().is_none());
        assert!(asm.feed(&packets[1]).unwrap().is_none());
        match asm.feed(&packets[2]) {
            Err(CtapHidError::InvalidSequence { expected, got }) => {
                assert!(expected != got, "seq mismatch detected: expected {expected}, got {got}");
            }
            other => panic!("Expected InvalidSequence error, got {:?}", other),
        }
    }

    #[test]
    fn test_max_payload() {
        let payload = vec![0xAA; MAX_PAYLOAD_SIZE];
        let msg = CtapHidMessage {
            cid: 0x00000001,
            cmd: CTAPHID_CBOR,
            payload: payload.clone(),
        };
        let packets = fragment(&msg).unwrap();

        let mut asm = CtapHidAssembler::new();
        let mut result = None;
        for pkt in &packets {
            result = asm.feed(pkt).unwrap();
        }
        let r = result.unwrap();
        assert_eq!(r.payload.len(), MAX_PAYLOAD_SIZE);
        assert_eq!(r.payload, payload);
    }

    #[test]
    fn test_payload_too_large() {
        let msg = CtapHidMessage {
            cid: 0x00000001,
            cmd: CTAPHID_CBOR,
            payload: vec![0; MAX_PAYLOAD_SIZE + 1],
        };
        assert!(matches!(
            fragment(&msg),
            Err(CtapHidError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn test_empty_payload() {
        let msg = CtapHidMessage {
            cid: 0x00000001,
            cmd: CTAPHID_CANCEL,
            payload: vec![],
        };

        let packets = fragment(&msg).unwrap();
        assert_eq!(packets.len(), 1);

        let mut asm = CtapHidAssembler::new();
        let result = asm.feed(&packets[0]).unwrap().unwrap();
        assert_eq!(result.cmd, CTAPHID_CANCEL);
        assert!(result.payload.is_empty());
    }

    #[test]
    fn test_keepalive_construction() {
        let ka = CtapHidMessage::keepalive(0x00000001, STATUS_UPNEEDED);
        assert_eq!(ka.cid, 0x00000001);
        assert_eq!(ka.cmd, CTAPHID_KEEPALIVE);
        assert_eq!(ka.payload, vec![STATUS_UPNEEDED]);
    }

    #[test]
    fn test_error_construction() {
        let err = CtapHidMessage::error(BROADCAST_CID, ERR_INVALID_CMD);
        assert_eq!(err.cid, BROADCAST_CID);
        assert_eq!(err.cmd, CTAPHID_ERROR);
        assert_eq!(err.payload, vec![ERR_INVALID_CMD]);
    }

    #[test]
    fn test_allowed_ctap2_commands() {
        assert!(is_allowed_ctap2_command(&[0x01])); // MakeCredential
        assert!(is_allowed_ctap2_command(&[0x02])); // GetAssertion
        assert!(is_allowed_ctap2_command(&[0x04])); // GetInfo
        assert!(is_allowed_ctap2_command(&[0x06])); // ClientPIN
        assert!(is_allowed_ctap2_command(&[0x08])); // GetNextAssertion
        assert!(is_allowed_ctap2_command(&[0x0B])); // Selection
        assert!(!is_allowed_ctap2_command(&[0x07])); // Reset - BLOCKED
        assert!(!is_allowed_ctap2_command(&[0x09])); // BioEnrollment - BLOCKED
        assert!(!is_allowed_ctap2_command(&[0x0A])); // CredentialManagement - BLOCKED
        assert!(!is_allowed_ctap2_command(&[0x0D])); // Config - BLOCKED
        assert!(!is_allowed_ctap2_command(&[]));
    }

    #[test]
    fn test_cid_mismatch() {
        let payload = vec![0u8; 200];
        let msg = CtapHidMessage {
            cid: 0x00000001,
            cmd: CTAPHID_CBOR,
            payload,
        };
        let packets = fragment(&msg).unwrap();

        let mut asm = CtapHidAssembler::new();
        assert!(asm.feed(&packets[0]).unwrap().is_none());

        // Corrupt CID in continuation packet
        let mut bad_pkt = packets[1];
        bad_pkt[0] = 0xFF;
        match asm.feed(&bad_pkt) {
            Err(CtapHidError::CidMismatch { .. }) => {}
            other => panic!("Expected CidMismatch, got {:?}", other),
        }
    }

    #[test]
    fn test_init_resets_assembler() {
        // Start assembling a multi-packet message
        let payload = vec![0u8; 200];
        let msg1 = CtapHidMessage {
            cid: 0x00000001,
            cmd: CTAPHID_CBOR,
            payload,
        };
        let packets1 = fragment(&msg1).unwrap();

        let mut asm = CtapHidAssembler::new();
        assert!(asm.feed(&packets1[0]).unwrap().is_none());
        assert!(asm.is_busy());

        // Send a new init packet (different message) -- should reset and start fresh
        let msg2 = CtapHidMessage {
            cid: 0x00000002,
            cmd: CTAPHID_CBOR,
            payload: vec![0x04],
        };
        let packets2 = fragment(&msg2).unwrap();
        let result = asm.feed(&packets2[0]).unwrap().unwrap();
        assert_eq!(result.cid, 0x00000002);
        assert_eq!(result.payload, vec![0x04]);
    }
}
