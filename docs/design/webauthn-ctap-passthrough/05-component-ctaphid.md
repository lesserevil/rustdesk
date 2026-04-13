# 05 - Component: CTAPHID Framing

**Assignee**: Developer B
**Estimated effort**: 3-5 days
**Dependencies**: None (pure protocol logic)
**New file**: `src/ctap_hid.rs`

## Background

CTAPHID is a framing protocol that wraps CTAP commands in 64-byte USB HID reports.
It handles message fragmentation (large messages split across multiple reports),
channel multiplexing (multiple logical channels on one HID device), and transport-level
commands (INIT, PING, KEEPALIVE, ERROR).

This component provides functions to:
1. Reassemble incoming CTAPHID packets into complete messages
2. Fragment outgoing messages into CTAPHID packets
3. Handle transport-level commands (INIT, PING) locally

It is used by both the remote CTAP service (Component 06) and the local
authenticator driver (Component 07).

## Constants

```rust
/// HID report size for FIDO devices (always 64 bytes)
pub const HID_REPORT_SIZE: usize = 64;

/// Max payload in an initialization packet (64 - 4 CID - 1 CMD - 2 LEN = 57)
pub const INIT_PACKET_DATA_SIZE: usize = 57;

/// Max payload in a continuation packet (64 - 4 CID - 1 SEQ = 59)
pub const CONT_PACKET_DATA_SIZE: usize = 59;

/// Maximum number of continuation packets (SEQ 0x00-0x7F = 128)
pub const MAX_CONT_PACKETS: usize = 128;

/// Maximum total payload size: 57 + 128*59 = 7609 bytes
pub const MAX_PAYLOAD_SIZE: usize = INIT_PACKET_DATA_SIZE + MAX_CONT_PACKETS * CONT_PACKET_DATA_SIZE;

/// Broadcast channel ID (used for CTAPHID_INIT)
pub const BROADCAST_CID: u32 = 0xFFFFFFFF;

// CTAPHID command bytes (without the 0x80 bit — that's added during framing)
pub const CTAPHID_PING: u8 = 0x01;
pub const CTAPHID_MSG: u8 = 0x03;
pub const CTAPHID_LOCK: u8 = 0x04;
pub const CTAPHID_INIT: u8 = 0x06;
pub const CTAPHID_WINK: u8 = 0x08;
pub const CTAPHID_CBOR: u8 = 0x10;
pub const CTAPHID_CANCEL: u8 = 0x11;
pub const CTAPHID_KEEPALIVE: u8 = 0x3B;
pub const CTAPHID_ERROR: u8 = 0x3F;

// KEEPALIVE status codes
pub const STATUS_PROCESSING: u8 = 0x01;
pub const STATUS_UPNEEDED: u8 = 0x02;

// Error codes
pub const ERR_INVALID_CMD: u8 = 0x01;
pub const ERR_INVALID_PAR: u8 = 0x02;
pub const ERR_INVALID_LEN: u8 = 0x03;
pub const ERR_INVALID_SEQ: u8 = 0x04;
pub const ERR_MSG_TIMEOUT: u8 = 0x05;
pub const ERR_CHANNEL_BUSY: u8 = 0x06;
pub const ERR_LOCK_REQUIRED: u8 = 0x0A;
pub const ERR_INVALID_CHANNEL: u8 = 0x0B;
pub const ERR_OTHER: u8 = 0x7F;
```

## Data Types

```rust
/// A fully reassembled CTAPHID message.
///
/// This is what you get after collecting all packets (init + continuations)
/// for a single CTAPHID transaction.
#[derive(Debug, Clone)]
pub struct CtapHidMessage {
    /// Channel ID (4 bytes, big-endian on wire)
    pub cid: u32,
    /// Command byte (without the 0x80 flag)
    pub cmd: u8,
    /// Complete payload (reassembled from all packets)
    pub payload: Vec<u8>,
}

/// State machine for reassembling multi-packet CTAPHID messages.
///
/// Feed it 64-byte HID reports one at a time. It tracks the current
/// message being assembled and emits complete CtapHidMessage values.
pub struct CtapHidAssembler {
    /// Current channel ID being assembled (None = idle)
    current_cid: Option<u32>,
    /// Expected command for this message
    current_cmd: u8,
    /// Total expected payload length
    expected_len: usize,
    /// Payload accumulated so far
    buffer: Vec<u8>,
    /// Next expected continuation sequence number
    next_seq: u8,
}
```

## API Design

### Assembler (packet → message)

```rust
impl CtapHidAssembler {
    /// Create a new assembler in idle state.
    pub fn new() -> Self;

    /// Feed a 64-byte HID report to the assembler.
    ///
    /// # Returns
    /// - `Ok(Some(msg))` — a complete message was assembled
    /// - `Ok(None)` — more packets needed to complete the message
    /// - `Err(CtapHidError)` — framing error (invalid seq, wrong CID, etc.)
    ///
    /// On error, the assembler resets to idle state. The caller should
    /// send a CTAPHID_ERROR response to the sender.
    pub fn feed(&mut self, report: &[u8; 64]) -> Result<Option<CtapHidMessage>, CtapHidError>;

    /// Reset the assembler to idle state, discarding any partial message.
    pub fn reset(&mut self);

    /// Returns true if the assembler is currently assembling a multi-packet message.
    pub fn is_busy(&self) -> bool;
}
```

### Fragmenter (message → packets)

```rust
/// Fragment a CTAPHID message into 64-byte HID reports.
///
/// Returns a Vec of exactly-64-byte arrays ready to write to a HID device.
///
/// # Arguments
/// - `msg`: The message to fragment
///
/// # Returns
/// One init packet followed by zero or more continuation packets.
///
/// # Errors
/// Returns error if payload exceeds MAX_PAYLOAD_SIZE (7609 bytes).
pub fn fragment(msg: &CtapHidMessage) -> Result<Vec<[u8; 64]>, CtapHidError>;
```

### Convenience Constructors

```rust
impl CtapHidMessage {
    /// Create a CTAPHID_INIT response.
    ///
    /// # Arguments
    /// - `nonce`: 8-byte nonce from the INIT request (echo it back)
    /// - `new_cid`: The channel ID to allocate
    /// - `capabilities`: Capability flags (use 0x04 for CBOR-only)
    pub fn init_response(nonce: &[u8; 8], new_cid: u32, capabilities: u8) -> Self;

    /// Create a CTAPHID_KEEPALIVE message.
    ///
    /// # Arguments
    /// - `cid`: Channel ID for this transaction
    /// - `status`: STATUS_PROCESSING (0x01) or STATUS_UPNEEDED (0x02)
    pub fn keepalive(cid: u32, status: u8) -> Self;

    /// Create a CTAPHID_ERROR message.
    ///
    /// # Arguments
    /// - `cid`: Channel ID (or BROADCAST_CID for general errors)
    /// - `error_code`: One of the ERR_* constants
    pub fn error(cid: u32, error_code: u8) -> Self;

    /// Create a CTAPHID_PING response (echo).
    ///
    /// # Arguments
    /// - `cid`: Channel ID
    /// - `data`: Data to echo back (copied from request)
    pub fn ping_response(cid: u32, data: Vec<u8>) -> Self;
}
```

### Error Type

```rust
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
```

## Implementation Guide

### Step 1: Parsing an initialization packet

Given a 64-byte report, determine if it's an init or continuation packet:

```rust
fn is_init_packet(report: &[u8; 64]) -> bool {
    // Byte 4 (CMD) has bit 7 set for init packets
    report[4] & 0x80 != 0
}

fn parse_cid(report: &[u8; 64]) -> u32 {
    u32::from_be_bytes([report[0], report[1], report[2], report[3]])
}
```

For init packets:
```rust
let cmd = report[4] & 0x7F;  // Strip the 0x80 bit to get the command
let total_len = ((report[5] as usize) << 8) | (report[6] as usize);
let data = &report[7..64];   // First 57 bytes of payload
```

For continuation packets:
```rust
let seq = report[4];          // Sequence number (0x00-0x7F)
let data = &report[5..64];   // 59 bytes of payload
```

### Step 2: Implementing the assembler

The assembler is a state machine with two states:
- **Idle**: Waiting for an init packet. Any continuation packet is an error.
- **Assembling**: Received init packet, waiting for continuation packets to
  complete the message.

```
                ┌──────────┐
                │          │
    init pkt    │   Idle   │◄──── reset() / message complete / error
   ──────────►  │          │
                └────┬─────┘
                     │
                     │ payload complete in init packet?
                     │
              ┌──────┴──────┐
              │ Yes         │ No
              ▼             ▼
        return Some(msg)    ┌────────────────┐
                            │  Assembling    │◄─── continuation pkt
                            │  (buffer data) │────► (more needed)
                            └───────┬────────┘
                                    │
                                    │ buffer.len() >= expected_len
                                    ▼
                              return Some(msg)
```

**Key implementation details:**
- When an init packet arrives during Assembling state, reset and start new message
  (the previous message was abandoned)
- Trim the final payload to `expected_len` (the last packet may have padding)
- Validate sequence numbers are strictly incrementing

### Step 3: Implementing the fragmenter

```rust
pub fn fragment(msg: &CtapHidMessage) -> Result<Vec<[u8; 64]>, CtapHidError> {
    if msg.payload.len() > MAX_PAYLOAD_SIZE {
        return Err(CtapHidError::PayloadTooLarge {
            size: msg.payload.len(),
            max: MAX_PAYLOAD_SIZE,
        });
    }

    let mut packets = Vec::new();
    let cid_bytes = msg.cid.to_be_bytes();
    let total_len = msg.payload.len();

    // Build init packet
    let mut init = [0u8; 64];
    init[0..4].copy_from_slice(&cid_bytes);
    init[4] = msg.cmd | 0x80;  // Set bit 7
    init[5] = ((total_len >> 8) & 0xFF) as u8;
    init[6] = (total_len & 0xFF) as u8;

    let first_chunk = total_len.min(INIT_PACKET_DATA_SIZE);
    init[7..7 + first_chunk].copy_from_slice(&msg.payload[..first_chunk]);
    packets.push(init);

    // Build continuation packets
    let mut offset = first_chunk;
    let mut seq: u8 = 0;
    while offset < total_len {
        let mut cont = [0u8; 64];
        cont[0..4].copy_from_slice(&cid_bytes);
        cont[4] = seq;  // Bit 7 is clear (continuation)

        let chunk = (total_len - offset).min(CONT_PACKET_DATA_SIZE);
        cont[5..5 + chunk].copy_from_slice(&msg.payload[offset..offset + chunk]);
        packets.push(cont);

        offset += chunk;
        seq += 1;
    }

    Ok(packets)
}
```

### Step 4: Convenience constructors

```rust
impl CtapHidMessage {
    pub fn init_response(nonce: &[u8; 8], new_cid: u32, capabilities: u8) -> Self {
        let mut payload = Vec::with_capacity(17);
        payload.extend_from_slice(nonce);               // bytes 0-7: echo nonce
        payload.extend_from_slice(&new_cid.to_be_bytes()); // bytes 8-11: new CID
        payload.push(2);                                 // byte 12: protocol version
        payload.push(0);                                 // byte 13: major version
        payload.push(0);                                 // byte 14: minor version
        payload.push(0);                                 // byte 15: build version
        payload.push(capabilities);                      // byte 16: capabilities

        Self {
            cid: BROADCAST_CID,
            cmd: CTAPHID_INIT,
            payload,
        }
    }

    pub fn keepalive(cid: u32, status: u8) -> Self {
        Self {
            cid,
            cmd: CTAPHID_KEEPALIVE,
            payload: vec![status],
        }
    }

    pub fn error(cid: u32, error_code: u8) -> Self {
        Self {
            cid,
            cmd: CTAPHID_ERROR,
            payload: vec![error_code],
        }
    }

    pub fn ping_response(cid: u32, data: Vec<u8>) -> Self {
        Self {
            cid,
            cmd: CTAPHID_PING,
            payload: data,
        }
    }
}
```

## Testing

This component has the highest test coverage requirements because it's pure logic
with no I/O dependencies.

### Test: Single-packet message (payload <= 57 bytes)

```rust
#[test]
fn test_single_packet_cbor() {
    // CTAP2 authenticatorGetInfo command (1 byte: 0x04)
    let msg = CtapHidMessage {
        cid: 0x00000001,
        cmd: CTAPHID_CBOR,
        payload: vec![0x04],
    };

    let packets = fragment(&msg).unwrap();
    assert_eq!(packets.len(), 1);

    // Verify init packet format
    assert_eq!(packets[0][0..4], [0x00, 0x00, 0x00, 0x01]); // CID
    assert_eq!(packets[0][4], 0x90);  // CBOR | 0x80
    assert_eq!(packets[0][5], 0x00);  // Length high byte
    assert_eq!(packets[0][6], 0x01);  // Length low byte
    assert_eq!(packets[0][7], 0x04);  // Payload

    // Reassemble
    let mut asm = CtapHidAssembler::new();
    let result = asm.feed(&packets[0]).unwrap();
    let reassembled = result.expect("Should be complete in one packet");
    assert_eq!(reassembled.cid, 0x00000001);
    assert_eq!(reassembled.cmd, CTAPHID_CBOR);
    assert_eq!(reassembled.payload, vec![0x04]);
}
```

### Test: Multi-packet message

```rust
#[test]
fn test_multi_packet_message() {
    // Create a payload larger than 57 bytes
    let payload: Vec<u8> = (0..200).map(|i| (i % 256) as u8).collect();
    let msg = CtapHidMessage {
        cid: 0xDEADBEEF,
        cmd: CTAPHID_CBOR,
        payload: payload.clone(),
    };

    let packets = fragment(&msg).unwrap();
    // 57 bytes in init + ceil((200-57)/59) = ceil(143/59) = 3 continuation = 4 total
    assert_eq!(packets.len(), 4);

    // Reassemble
    let mut asm = CtapHidAssembler::new();
    assert!(asm.feed(&packets[0]).unwrap().is_none());
    assert!(asm.feed(&packets[1]).unwrap().is_none());
    assert!(asm.feed(&packets[2]).unwrap().is_none());
    let result = asm.feed(&packets[3]).unwrap().unwrap();
    assert_eq!(result.cid, 0xDEADBEEF);
    assert_eq!(result.cmd, CTAPHID_CBOR);
    assert_eq!(result.payload, payload);
}
```

### Test: INIT request/response

```rust
#[test]
fn test_init_flow() {
    // Simulate browser INIT request
    let nonce: [u8; 8] = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
    let init_req = CtapHidMessage {
        cid: BROADCAST_CID,
        cmd: CTAPHID_INIT,
        payload: nonce.to_vec(),
    };

    let packets = fragment(&init_req).unwrap();
    assert_eq!(packets.len(), 1);

    // Parse it
    let mut asm = CtapHidAssembler::new();
    let req = asm.feed(&packets[0]).unwrap().unwrap();
    assert_eq!(req.cid, BROADCAST_CID);
    assert_eq!(req.cmd, CTAPHID_INIT);
    assert_eq!(req.payload.len(), 8);

    // Build response
    let resp = CtapHidMessage::init_response(&nonce, 0x00000001, 0x04);
    assert_eq!(resp.payload.len(), 17);
    assert_eq!(&resp.payload[0..8], &nonce);           // Nonce echo
    assert_eq!(&resp.payload[8..12], &[0, 0, 0, 1]);  // Allocated CID
    assert_eq!(resp.payload[12], 2);                    // Protocol version
    assert_eq!(resp.payload[16], 0x04);                 // CBOR capability
}
```

### Test: Sequence error

```rust
#[test]
fn test_sequence_error() {
    let payload = vec![0u8; 200];
    let msg = CtapHidMessage {
        cid: 0x00000001,
        cmd: CTAPHID_CBOR,
        payload,
    };

    let mut packets = fragment(&msg).unwrap();

    // Corrupt: swap continuation packets 1 and 2
    packets.swap(2, 3);

    let mut asm = CtapHidAssembler::new();
    assert!(asm.feed(&packets[0]).unwrap().is_none()); // Init OK
    assert!(asm.feed(&packets[1]).unwrap().is_none()); // Cont 0 OK
    // Cont 2 arrives when expecting 1 — should error
    match asm.feed(&packets[2]) {
        Err(CtapHidError::InvalidSequence { expected: 1, got: 2 }) => {} // Correct
        other => panic!("Expected InvalidSequence error, got {:?}", other),
    }
}
```

### Test: Max payload boundary

```rust
#[test]
fn test_max_payload() {
    let payload = vec![0xAA; MAX_PAYLOAD_SIZE];
    let msg = CtapHidMessage {
        cid: 0x00000001,
        cmd: CTAPHID_CBOR,
        payload: payload.clone(),
    };
    let packets = fragment(&msg).unwrap();

    // Reassemble
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
```

## File Placement

Place in `libs/ctap-common/src/ctap_hid.rs` as part of the shared `ctap-common`
library crate. This crate is used by:

- The main RustDesk binary (remote service + native local driver)
- The standalone CTAP Companion App (Component 13, for the web client)

```toml
# libs/ctap-common/Cargo.toml
[package]
name = "ctap-common"
version = "0.1.0"
edition = "2021"

[dependencies]
thiserror = "1"
```

The crate has no OS-specific dependencies — CTAPHID framing is pure protocol logic.
OS-specific gating (`#[cfg(target_os = "linux")]`) is applied by the consumers,
not by `ctap-common` itself.

## Acceptance Criteria

- [ ] `CtapHidAssembler` correctly reassembles single-packet messages
- [ ] `CtapHidAssembler` correctly reassembles multi-packet messages
- [ ] `fragment()` correctly produces init + continuation packets
- [ ] Round-trip: `fragment()` → `assembler.feed()` produces identical message
- [ ] Sequence errors are detected and reported
- [ ] CID mismatch errors are detected and reported
- [ ] Payload too large is rejected
- [ ] INIT response constructor produces correct 17-byte payload
- [ ] KEEPALIVE constructor produces correct 1-byte payload
- [ ] ERROR constructor produces correct 1-byte payload
- [ ] All tests pass
- [ ] No unsafe code (this component should be 100% safe Rust)
