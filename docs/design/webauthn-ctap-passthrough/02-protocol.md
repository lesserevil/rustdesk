# 02 - Protocol Specification

## Overview

This document defines the wire formats and timing requirements for CTAP passthrough.
There are three protocol layers:

1. **CTAPHID** - between browser and virtual device / between local driver and physical key
2. **CtapFrame** - RustDesk protobuf messages between remote service and local driver
3. **Capability negotiation** - during RustDesk session setup

## Layer 1: CTAPHID (USB HID Transport)

Reference: FIDO Alliance CTAP 2.1, Section 11.2 (USB HID)

### HID Report Format

All CTAPHID communication uses **64-byte HID reports** with no Report ID prefix
(the FIDO HID report descriptor does not define Report IDs).

### Initialization Packet

The first packet of every CTAPHID message:

```
Byte   Length   Field      Description
────   ──────   ─────      ───────────
0-3    4        CID        Channel Identifier (big-endian uint32)
4      1        CMD        Command byte with bit 7 SET (cmd | 0x80)
5      1        BCNTH      Payload length, high byte
6      1        BCNTL      Payload length, low byte
7-63   57       DATA       First fragment of payload
```

Total payload length = `(BCNTH << 8) | BCNTL`. Max payload per init packet: 57 bytes.

### Continuation Packet

If the payload exceeds 57 bytes, continuation packets follow:

```
Byte   Length   Field      Description
────   ──────   ─────      ───────────
0-3    4        CID        Channel Identifier (same as init)
4      1        SEQ        Sequence number 0x00-0x7F (bit 7 CLEAR)
5-63   59       DATA       Next fragment of payload
```

SEQ starts at 0 and increments. Max continuation packets: 128.
Max total payload: `57 + (128 * 59) = 7,609 bytes`.

### CTAPHID Commands

| Command | Value | Direction | Payload | Notes |
|---------|-------|-----------|---------|-------|
| CTAPHID_PING | 0x01 | Both | Echo data | Diagnostic, handle locally |
| CTAPHID_MSG | 0x03 | Both | U2F APDU | Legacy CTAP1, handle locally as error |
| CTAPHID_LOCK | 0x04 | H→D | Lock duration (1 byte, seconds) | Optional, not supported |
| CTAPHID_INIT | 0x06 | Both | 8-byte nonce / 17-byte response | Channel allocation, handle locally |
| CTAPHID_WINK | 0x08 | H→D | Empty | Optional, not supported |
| CTAPHID_CBOR | 0x10 | Both | CTAP2 CBOR | **Forward through tunnel** |
| CTAPHID_CANCEL | 0x11 | H→D | Empty | Forward as cancellation signal |
| CTAPHID_KEEPALIVE | 0x3B | D→H | 1-byte status | Generated locally by remote service |
| CTAPHID_ERROR | 0x3F | D→H | 1-byte error code | Generated locally on errors |

### CTAPHID_INIT Protocol

**Request** (sent on broadcast CID `0xFFFFFFFF`):
```
Byte   Length   Field   Description
0-7    8        NONCE   Random 8-byte nonce
```

**Response**:
```
Byte   Length   Field              Description
0-7    8        NONCE              Echo of request nonce
8-11   4        CID                Newly allocated channel ID
12     1        PROTOCOL_VERSION   Always 2
13     1        MAJOR_VERSION      Device version major (set to 0)
14     1        MINOR_VERSION      Device version minor (set to 0)
15     1        BUILD_VERSION      Device version build (set to 0)
16     1        CAPABILITIES       Capability flags
```

**Capabilities byte for our virtual device**: `0x04` (CAPABILITY_CBOR).
We set CAPABILITY_CBOR to indicate CTAP2 support. We do NOT set CAPABILITY_NMSG
(0x08) because we want to reject CTAPHID_MSG with an error rather than silently
accept it.

### KEEPALIVE Status Codes

| Status | Value | Meaning |
|--------|-------|---------|
| STATUS_PROCESSING | 0x01 | Authenticator is busy processing |
| STATUS_UPNEEDED | 0x02 | Waiting for user presence (touch) |

### Error Codes

| Error | Value | When to use |
|-------|-------|-------------|
| ERR_INVALID_CMD | 0x01 | Unrecognized command received |
| ERR_INVALID_PAR | 0x02 | Invalid parameters |
| ERR_INVALID_LEN | 0x03 | Payload length mismatch |
| ERR_INVALID_SEQ | 0x04 | Out-of-order continuation packet |
| ERR_MSG_TIMEOUT | 0x05 | Transaction timed out |
| ERR_CHANNEL_BUSY | 0x06 | CID already has an active transaction |
| ERR_LOCK_REQUIRED | 0x0A | Operation requires LOCK (unsupported) |
| ERR_INVALID_CHANNEL | 0x0B | CID not recognized |
| ERR_OTHER | 0x7F | Unspecified error |

### Timing Requirements

| Parameter | Value | Notes |
|-----------|-------|-------|
| KEEPALIVE interval | 100ms | Remote service sends while waiting for tunnel response |
| Transaction timeout (browser side) | ~30s | Browser gives up if no response/keepalive |
| CTAPHID packet timeout | 500ms | Between init and first continuation packet |
| Inter-packet timeout | 500ms | Between consecutive continuation packets |

## Layer 2: CtapFrame (RustDesk Tunnel Protocol)

### Protobuf Definition

```protobuf
message CtapFrame {
    uint32 command = 1;      // CTAPHID command byte (0x10 for CBOR, 0x11 for CANCEL)
    bytes  payload = 2;      // Raw CTAP2 CBOR data (no CTAPHID framing)
    bool   is_response = 3;  // false = request (remote→local), true = response (local→remote)
    uint32 error_code = 4;   // Non-zero if this is an error response
}
```

**What goes in `payload`:**
- For `CTAPHID_CBOR` (command=0x10): The raw CTAP2 CBOR bytes. This is the
  reassembled payload from CTAPHID packets, starting with the CTAP2 command byte
  (e.g., 0x01 for makeCredential, 0x02 for getAssertion) followed by the CBOR map.
- For `CTAPHID_CANCEL` (command=0x11): Empty payload.
- For error responses: Empty payload, `error_code` set to a CTAP2 status code.

**What does NOT go in the tunnel:**
- CTAPHID_INIT: Handled entirely by the remote virtual device
- CTAPHID_PING: Handled entirely by the remote virtual device
- CTAPHID_KEEPALIVE: Generated locally by the remote service
- CTAPHID framing (64-byte chunking, CID, SEQ): Stripped before tunneling
- CID (Channel ID): Not needed in tunnel since we support single-transaction

### Message Flow Direction

```
Remote (browser request)  ──►  CtapFrame{is_response=false}  ──►  Local (physical key)
Remote (browser gets answer) ◄── CtapFrame{is_response=true}  ◄──  Local (key responds)
```

### Error Propagation

If the local driver encounters an error (no key found, user cancelled, timeout):
```protobuf
CtapFrame {
    command: 0x10,        // CBOR
    payload: b"",         // empty
    is_response: true,
    error_code: 0x27,     // e.g., CTAP2_ERR_OPERATION_DENIED
}
```

The remote service translates this to a CTAPHID_ERROR or CTAP2 error CBOR response.

### Timing Over the Tunnel

The remote service sends KEEPALIVE to the browser every 100ms while waiting for
a CtapFrame response. The tunnel adds latency (typically 10-200ms depending on
relay vs. direct connection). This means:

- The local driver should respond within ~25 seconds to leave margin for the
  browser's ~30-second timeout
- The remote service MUST NOT wait for the tunnel response before sending the
  first KEEPALIVE (the browser expects KEEPALIVE within 500ms of the CBOR request)

## Layer 3: Capability Negotiation

### During PeerInfo Exchange

The remote server advertises CTAP support in the `PeerInfo` message sent during
login. This uses the existing `SupportedFeatures` mechanism:

```protobuf
// In PeerInfo (already exists, add field):
message PeerInfo {
    // ... existing fields ...
    bool ctap_passthrough_supported = N;  // Next available field number
}
```

### Client Opt-In

After receiving PeerInfo with `ctap_passthrough_supported=true`, the client
sends a `Misc` message to enable the feature:

```protobuf
// In Misc oneof (already exists, add variant):
message CtapControl {
    bool enabled = 1;     // true = start CTAP service, false = stop
}
```

The remote server only creates the virtual FIDO device after receiving
`CtapControl{enabled: true}`.

### Permission Integration

The CTAP permission follows the same pattern as keyboard/clipboard/file:

```protobuf
// In PermissionInfo.Permission enum (existing, add value):
enum Permission {
    Keyboard = 0;
    Clipboard = 2;
    Audio = 3;
    File = 4;
    Restart = 5;
    Recording = 6;
    BlockInput = 7;
    Ctap = 8;           // New
}
```

## CTAP2 Command Bytes (Reference)

These are the first byte of the CTAP2 CBOR payload (inside CTAPHID_CBOR).
The tunnel forwards these opaquely, but they are listed here for debugging:

| Command | Byte | Description |
|---------|------|-------------|
| authenticatorMakeCredential | 0x01 | Create a new credential |
| authenticatorGetAssertion | 0x02 | Sign a challenge (login) |
| authenticatorGetInfo | 0x04 | Get authenticator capabilities |
| authenticatorClientPIN | 0x06 | PIN management |
| authenticatorReset | 0x07 | Factory reset (MUST NOT forward) |
| authenticatorGetNextAssertion | 0x08 | Get additional assertions |
| authenticatorBioEnrollment | 0x09 | Biometric enrollment |
| authenticatorCredentialManagement | 0x0A | Manage stored credentials |
| authenticatorSelection | 0x0B | Select this authenticator |
| authenticatorLargeBlobs | 0x0C | Large blob operations |
| authenticatorConfig | 0x0D | Configure authenticator |

**Security filter**: The remote service SHOULD reject `authenticatorReset` (0x07)
to prevent a remote attacker from resetting the user's security key. See
[12-security.md](12-security.md) for full threat analysis.
