# 03 - Component: Protobuf Messages

**Assignee**: Developer A
**Estimated effort**: 1-2 days
**Dependencies**: None (other components depend on this)
**Files to modify**:
- `libs/hbb_common/protos/message.proto`

## Background

RustDesk uses Protocol Buffers (proto3) for all peer-to-peer messages. The proto
files live in `libs/hbb_common/protos/` and are compiled at build time by
`protobuf-codegen` (pure Rust, no C++ compiler needed). The build script is at
`libs/hbb_common/build.rs`.

The generated Rust code is included via:
```rust
// libs/hbb_common/src/protos/mod.rs
include!(concat!(env!("OUT_DIR"), "/protos/mod.rs"));
```

And re-exported as:
```rust
// libs/hbb_common/src/lib.rs
pub use protos::message as message_proto;
```

Application code imports types with `use hbb_common::message_proto::*;`.

## Requirements

### R1: Add CtapFrame message

Add a new message type for CTAP passthrough frames.

**Proto definition to add:**
```protobuf
message CtapFrame {
    uint32 command = 1;
    bytes  payload = 2;
    bool   is_response = 3;
    uint32 error_code = 4;
}
```

**Field semantics:**

| Field | Type | Description |
|-------|------|-------------|
| `command` | uint32 | CTAPHID command byte. Will be `0x10` (CBOR) or `0x11` (CANCEL) in practice. Stored as uint32 for protobuf efficiency (varint). |
| `payload` | bytes | Raw CTAP2 CBOR payload. Empty for CANCEL and error frames. Max size in practice: ~7KB (CTAPHID max payload). |
| `is_response` | bool | `false` = request flowing remote→local. `true` = response flowing local→remote. |
| `error_code` | uint32 | CTAP2 error status code. `0` = no error. Non-zero only when `is_response=true` and the local driver could not complete the request. |

### R2: Add CtapFrame to Message oneof

Add `CtapFrame` as a new variant in the top-level `Message` union.

**Current state** (last field number in `Message` oneof is 32 for `terminal_response`):
```protobuf
message Message {
    oneof union {
        // ... existing fields 3-32 ...
    }
}
```

**Change**:
```protobuf
message Message {
    oneof union {
        // ... existing fields 3-32 ...
        CtapFrame ctap_frame = 33;
    }
}
```

### R3: Add CtapControl message

Add a control message for the client to enable/disable CTAP passthrough during
a session. This goes inside the existing `Misc` message's oneof:

```protobuf
message CtapControl {
    bool enabled = 1;
}
```

**Current state of Misc** (has many oneof variants):
```protobuf
message Misc {
    oneof union {
        // ... many existing variants ...
    }
}
```

**Change**: Add `CtapControl ctap_control = N;` to the Misc oneof, where N is
the next available field number. To find N: open `message.proto`, find the `Misc`
message, look at the highest field number in its oneof, and use N = that + 1.

### R4: Add ctap_passthrough_supported to PeerInfo

The remote server needs to advertise that it supports CTAP passthrough.

**Current state**: `PeerInfo` has many fields. Find the last field number.

**Change**: Add `bool ctap_passthrough_supported = N;` where N is the next
available field number in PeerInfo.

### R5: Add Ctap to PermissionInfo.Permission enum

**Current state**:
```protobuf
message PermissionInfo {
    enum Permission {
        Keyboard = 0;
        Clipboard = 2;
        Audio = 3;
        File = 4;
        Restart = 5;
        Recording = 6;
        BlockInput = 7;
    }
    Permission permission = 1;
    bool enabled = 2;
}
```

**Change**: Add `Ctap = 8;` to the Permission enum.

## Step-by-Step Implementation

### Step 1: Find the proto file

Open `libs/hbb_common/protos/message.proto`. This file is ~800 lines and contains
all message definitions.

### Step 2: Add the CtapFrame message

Add the following BEFORE the `Message` definition (convention: message definitions
come before they are referenced):

```protobuf
message CtapFrame {
    uint32 command = 1;
    bytes  payload = 2;
    bool   is_response = 3;
    uint32 error_code = 4;
}
```

### Step 3: Add CtapControl message

Add near the other small message definitions:

```protobuf
message CtapControl {
    bool enabled = 1;
}
```

### Step 4: Modify the Message oneof

Find the `Message` message definition. It will look like:
```protobuf
message Message {
    oneof union {
        SignedId signed_id = 3;
        // ... many fields ...
        TerminalResponse terminal_response = 32;
    }
}
```

Add after the last field:
```protobuf
        CtapFrame ctap_frame = 33;
```

**CRITICAL**: Use the next sequential field number. If someone has added fields
since this document was written, use the number after whatever is currently last.
Never reuse a field number.

### Step 5: Modify the Misc oneof

Find the `Misc` message definition and add `CtapControl ctap_control = N;` to
its oneof, using the next available field number.

### Step 6: Modify PeerInfo

Find `PeerInfo` and add `bool ctap_passthrough_supported = N;` with the next
available field number.

### Step 7: Modify PermissionInfo.Permission

Find `PermissionInfo` and add `Ctap = 8;` to the `Permission` enum.

### Step 8: Build and verify

```bash
cd libs/hbb_common
cargo build
```

This runs `build.rs` which invokes `protobuf-codegen`. If successful, the
generated Rust types will include:

- `CtapFrame` struct with `command`, `payload`, `is_response`, `error_code` fields
- `CtapControl` struct with `enabled` field
- `message::Union::CtapFrame(CtapFrame)` variant
- `misc::Union::CtapControl(CtapControl)` variant
- `permission_info::Permission::Ctap` enum variant

### Step 9: Verify usage in code

Write a quick test in `libs/hbb_common/src/lib.rs` or a test file:

```rust
#[cfg(test)]
mod tests {
    use crate::message_proto::*;

    #[test]
    fn test_ctap_frame_creation() {
        let mut frame = CtapFrame::new();
        frame.command = 0x10;
        frame.payload = vec![0x02, 0x01, 0x02]; // getAssertion example
        frame.is_response = false;
        frame.error_code = 0;

        let mut msg = Message::new();
        msg.set_ctap_frame(frame);

        // Round-trip through serialization
        let bytes = msg.write_to_bytes().unwrap();
        let msg2 = Message::parse_from_bytes(&bytes).unwrap();

        match msg2.union {
            Some(message::Union::CtapFrame(f)) => {
                assert_eq!(f.command, 0x10);
                assert_eq!(f.payload, vec![0x02, 0x01, 0x02]);
                assert_eq!(f.is_response, false);
                assert_eq!(f.error_code, 0);
            }
            _ => panic!("Expected CtapFrame"),
        }
    }

    #[test]
    fn test_ctap_control_in_misc() {
        let mut ctrl = CtapControl::new();
        ctrl.enabled = true;

        let mut misc = Misc::new();
        misc.set_ctap_control(ctrl);

        let mut msg = Message::new();
        msg.set_misc(misc);

        let bytes = msg.write_to_bytes().unwrap();
        let msg2 = Message::parse_from_bytes(&bytes).unwrap();

        match msg2.union {
            Some(message::Union::Misc(m)) => {
                match m.union {
                    Some(misc::Union::CtapControl(c)) => {
                        assert!(c.enabled);
                    }
                    _ => panic!("Expected CtapControl"),
                }
            }
            _ => panic!("Expected Misc"),
        }
    }

    #[test]
    fn test_ctap_permission() {
        let mut pi = PermissionInfo::new();
        pi.permission = permission_info::Permission::Ctap.into();
        pi.enabled = true;

        assert_eq!(pi.permission.value(), 8);
    }
}
```

## Acceptance Criteria

- [ ] `cargo build` succeeds in `libs/hbb_common` with no errors
- [ ] `cargo build` succeeds in the root workspace (no downstream breakage)
- [ ] `CtapFrame` can be serialized and deserialized round-trip
- [ ] `CtapFrame` is accessible as `message::Union::CtapFrame`
- [ ] `CtapControl` is accessible as `misc::Union::CtapControl`
- [ ] `permission_info::Permission::Ctap` exists with value 8
- [ ] `PeerInfo` has `ctap_passthrough_supported` field
- [ ] All three unit tests above pass
- [ ] No existing tests are broken (`cargo test` in `libs/hbb_common`)
