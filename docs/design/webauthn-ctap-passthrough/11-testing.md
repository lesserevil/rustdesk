# 11 - Testing Plan

**Audience**: All developers
**Dependencies**: All components

## Test Levels

```mermaid
flowchart BT
    A[Unit Tests] --> B[Integration Tests]
    B --> C[System Tests]
    C --> D[Manual Acceptance Tests]
```

## Unit Tests

These run in CI without special hardware or permissions.

### CTAPHID Framing (Component 05)

| Test | Description | File |
|------|-------------|------|
| `test_single_packet_cbor` | Fragment and reassemble a 1-byte payload | `src/ctap_hid.rs` |
| `test_multi_packet_message` | Fragment and reassemble 200-byte payload | `src/ctap_hid.rs` |
| `test_max_payload` | Fragment and reassemble 7609-byte payload | `src/ctap_hid.rs` |
| `test_payload_too_large` | Reject payload > 7609 bytes | `src/ctap_hid.rs` |
| `test_init_flow` | INIT request/response construction | `src/ctap_hid.rs` |
| `test_keepalive_construction` | KEEPALIVE message format | `src/ctap_hid.rs` |
| `test_error_construction` | ERROR message format | `src/ctap_hid.rs` |
| `test_sequence_error` | Detect out-of-order continuation packets | `src/ctap_hid.rs` |
| `test_cid_mismatch` | Detect CID mismatch in continuation | `src/ctap_hid.rs` |
| `test_init_resets_assembler` | New INIT during assembly resets state | `src/ctap_hid.rs` |
| `test_round_trip_random` | Fragment and reassemble random payloads (proptest) | `src/ctap_hid.rs` |

### Protobuf Messages (Component 03)

| Test | Description | File |
|------|-------------|------|
| `test_ctap_frame_creation` | Create and serialize CtapFrame | `libs/hbb_common` |
| `test_ctap_frame_round_trip` | Serialize and deserialize CtapFrame | `libs/hbb_common` |
| `test_ctap_control_in_misc` | CtapControl inside Misc message | `libs/hbb_common` |
| `test_ctap_permission_value` | Permission::Ctap has value 8 | `libs/hbb_common` |

### Configuration (Component 10)

| Test | Description | File |
|------|-------------|------|
| `test_ctap_disabled_by_default` | Default config has CTAP off | `src/server/connection.rs` |
| `test_ctap_option_key_exists` | OPTION_ENABLE_CTAP is defined | `libs/hbb_common/src/config.rs` |

## Integration Tests

These require a Linux system with `/dev/uhid` access (run as root or with
appropriate udev rules). Mark with `#[ignore]` for CI — run manually or in
a privileged CI job.

### Virtual Device — Linux (Component 04, Part A)

Require `/dev/uhid` access (run as root or with udev rules). Mark with `#[ignore]`.

| Test | Description |
|------|-------------|
| `test_uhid_create_destroy` | Create a virtual FIDO device, verify it appears in `/sys/class/hidraw/`, destroy it, verify it disappears |
| `test_uhid_report_descriptor` | Create device, read its report descriptor via ioctl on the hidraw node, verify it matches `FIDO_HID_REPORT_DESCRIPTOR` |
| `test_uhid_write_read` | Create device, open the hidraw node, write an output report, read it from uhid as `UHID_OUTPUT` |
| `test_uhid_input_report` | Create device, write `UHID_INPUT2` from uhid, read it from the hidraw node |

### Virtual Device — Windows (Component 04, Part B)

Require VHF driver installed. Mark with `#[ignore]`.

| Test | Description |
|------|-------------|
| `test_vhf_driver_available` | `is_driver_available()` returns true when driver is installed |
| `test_vhf_create_destroy` | Create virtual FIDO device, verify it appears in Device Manager HID class, destroy it |
| `test_vhf_ioctl_read_write` | Create device, write output report via HID class → VHF, read via IOCTL, write input report via IOCTL, verify on HID class side |
| `test_vhf_report_descriptor` | Create device, query HID report descriptor via `HidD_GetPreparsedData`, verify FIDO usage page |

### Virtual Device — macOS (Component 04, Part C)

Require DriverKit extension installed and approved. Mark with `#[ignore]`.

| Test | Description |
|------|-------------|
| `test_driverkit_available` | `is_driver_available()` returns true when extension is approved |
| `test_driverkit_create_destroy` | Open user client, verify virtual FIDO device appears in `ioreg`, close connection |
| `test_driverkit_read_write` | Open user client, send output report via IOKit HID → extension, read via user client method 0, write input report via user client method 1, verify on IOKit HID side |
| `test_driverkit_report_descriptor` | Verify FIDO usage page via IOKit HID device properties |

### CTAPHID Over Virtual Device (Components 04 + 05, all platforms)

| Test | Description |
|------|-------------|
| `test_ctaphid_init_over_virtual_device` | Create virtual device (platform-appropriate), open HID handle, send INIT request, handle in code, send response, read on HID handle, verify correct response |
| `test_ctaphid_cbor_over_virtual_device` | Send a CBOR command through HID → virtual device, reassemble, verify payload matches |
| `test_ctaphid_multi_packet_over_virtual_device` | Send a large CBOR command that requires continuation packets |

### Remote Service (Component 06)

| Test | Description |
|------|-------------|
| `test_service_init_handling` | Start service, send INIT via hidraw, verify response |
| `test_service_cbor_forwarding` | Start service, send CBOR via hidraw, verify CtapFrame appears on tx channel |
| `test_service_keepalive` | Start service, send CBOR, delay response, verify KEEPALIVEs on hidraw |
| `test_service_response_relay` | Start service, send CBOR, inject response on rx channel, verify on hidraw |
| `test_service_timeout` | Start service, send CBOR, don't respond, verify ERROR after timeout |
| `test_service_cancel` | Start service, send CBOR, send CANCEL, verify forwarded |
| `test_service_reset_blocked` | Send authenticatorReset (0x07) via CBOR, verify rejected |

### Local Driver (Component 07)

These require a physical FIDO2 security key connected via USB.

| Test | Description |
|------|-------------|
| `test_enumerate_fido_device` | Verify `open_first()` finds the connected key |
| `test_get_info` | Send authenticatorGetInfo (0x04), verify valid CBOR response |
| `test_channel_allocation` | Allocate a channel via INIT, verify valid CID |

## System Tests (End-to-End)

These require TWO Linux machines (or VMs) with RustDesk installed, plus a
physical FIDO2 security key.

### Test Environment Setup

```mermaid
flowchart LR
    subgraph Local["Local Machine (Linux, Windows, or macOS)"]
        KEY[USB Security Key]
        CLIENT[RustDesk Client]
    end

    subgraph Remote["Remote Machine (Linux, Windows, or macOS)"]
        BROWSER[Chrome/Firefox/Edge/Safari]
        SERVER[RustDesk Server]
    end

    CLIENT <-->|RustDesk Connection| SERVER
```

**Prerequisites**:
- RustDesk built with CTAP feature enabled on both machines
- Remote setup:
  - **Linux**: udev rules installed (for /dev/uhid access)
  - **Windows**: VHF driver installed and signed
  - **macOS**: DriverKit extension installed and approved in System Settings
- Client setup:
  - **Linux**: udev rules for hidraw access to FIDO key
  - **Windows**: No special setup (USB HID accessible by default)
  - **macOS**: No special setup (USB HID accessible by default)
- Physical FIDO2 security key (YubiKey 5, SoloKey, or similar)
- Chrome 90+ / Firefox 100+ / Edge 100+ on remote machine
- A WebAuthn test account (e.g., webauthn.io, passkeys.io, or GitHub with
  security key configured)

### Platform Test Matrix

E2E tests should be run across the following combinations:

| Remote OS | Client OS | Client Type | Priority |
|-----------|-----------|-------------|----------|
| Linux | Linux | Native | P0 |
| Linux | Windows | Native | P0 |
| Linux | macOS | Native | P0 |
| Windows | Windows | Native | P0 |
| Windows | macOS | Native | P0 |
| macOS | macOS | Native | P1 |
| macOS | Windows | Native | P1 |
| macOS | Linux | Native | P1 |
| Linux | Any | Web + Companion | P1 |
| Windows | Any | Web + Companion | P1 |
| macOS | Any | Web + Companion | P2 |

### E2E-1: Basic Authentication Flow

**Steps**:
1. Connect local to remote via RustDesk
2. Enable CTAP passthrough in both client and server settings
3. On remote browser, navigate to webauthn.io
4. Register a new credential using the security key
5. Verify: prompt appears on local machine
6. Tap the security key
7. Verify: registration succeeds on the remote browser
8. Log out of webauthn.io
9. Log back in using the same credential
10. Verify: prompt appears again
11. Tap the security key
12. Verify: authentication succeeds

**Expected result**: Full registration + authentication cycle works through the tunnel.

### E2E-2: User Cancellation

**Steps**:
1. Connect and enable CTAP
2. Trigger WebAuthn authentication on remote browser
3. When prompt appears, click "Cancel"
4. Verify: remote browser shows authentication failure
5. Verify: prompt dismisses cleanly

### E2E-3: Timeout

**Steps**:
1. Connect and enable CTAP
2. Trigger WebAuthn authentication on remote browser
3. Do NOT touch the security key
4. Wait 30 seconds
5. Verify: remote browser shows timeout error
6. Verify: prompt dismisses cleanly

### E2E-4: Key Not Connected

**Steps**:
1. Connect and enable CTAP (but do NOT have a security key plugged in)
2. Trigger WebAuthn authentication on remote browser
3. Verify: remote browser shows error (no authenticator available)
4. Verify: no prompt appears on local machine (or prompt shows error)

### E2E-5: Permission Disabled

**Steps**:
1. Connect to remote machine
2. Disable CTAP passthrough in server settings
3. Trigger WebAuthn authentication on remote browser
4. Verify: no prompt appears (browser uses local authenticator or fails)

### E2E-6: Mid-Session Toggle

**Steps**:
1. Connect with CTAP enabled
2. Verify: virtual FIDO device exists on remote (`ls /sys/class/hidraw/`)
3. Toggle CTAP off in toolbar
4. Verify: virtual FIDO device is removed
5. Toggle CTAP on again
6. Verify: virtual FIDO device reappears
7. Trigger WebAuthn authentication — should still work

### E2E-7: Connection Disconnect

**Steps**:
1. Connect with CTAP enabled
2. Verify: virtual FIDO device exists on remote
3. Disconnect the RustDesk session
4. Verify: virtual FIDO device is removed from remote
5. Verify: no orphaned processes or file descriptors

### E2E-8: Multiple Browser Tabs

**Steps**:
1. Connect with CTAP enabled
2. Open two tabs with WebAuthn sites on remote browser
3. Trigger authentication on tab 1
4. Before completing, switch to tab 2 and trigger authentication there
5. Verify: tab 2 gets ERR_CHANNEL_BUSY or is queued
6. Complete authentication on tab 1
7. Verify: tab 2 can then authenticate

### E2E-9: Latency Test

**Steps**:
1. Connect via relay (not direct) for higher latency
2. Trigger WebAuthn authentication
3. Verify: authentication still succeeds (within 30s timeout)
4. Measure time from tap to completion

**Expected**: < 2 seconds additional latency from the tunnel.

### E2E-10: Web Client + Companion — Full Flow

**Prerequisites**: RustDesk web client in browser, CTAP Companion App running locally,
physical FIDO key connected.

**Steps**:
1. Open the RustDesk web client and connect to the remote machine
2. Enable CTAP passthrough
3. On remote browser, navigate to webauthn.io
4. Register a new credential
5. Verify: "Tap your security key" prompt appears in the web client
6. Tap the key
7. Verify: registration succeeds
8. Log out, log back in, tap key again
9. Verify: authentication succeeds

### E2E-11: Web Client — Companion Not Running

**Steps**:
1. Ensure CTAP Companion App is NOT running
2. Open RustDesk web client and connect to remote machine
3. Enable CTAP passthrough, trigger WebAuthn on remote
4. Verify: web client shows "Companion app required" guidance with download link
5. Verify: remote browser receives an authentication error (not a hang)

### E2E-12: Web Client — Companion Disconnect Mid-Transaction

**Steps**:
1. Web client + companion running, trigger WebAuthn authentication
2. While "Tap your key" prompt is visible, kill the companion process
3. Verify: web client detects disconnection, dismisses prompt
4. Verify: remote browser receives an error (not a hang)
5. Restart companion, trigger another authentication
6. Verify: web client reconnects and authentication works

## Performance Tests

| Metric | Target | How to measure |
|--------|--------|----------------|
| CTAP transaction latency overhead | < 200ms (direct), < 500ms (relay) | Timestamp at service entry/exit |
| KEEPALIVE jitter | < 50ms | Capture hidraw traffic, measure interval variance |
| Virtual device creation time | < 100ms | Timestamp around uhid create |
| Memory overhead of CTAP service | < 1MB | Process memory before/after enabling |

## Browser Compatibility Matrix (Remote Side)

| Browser | Version | Remote OS | FIDO Discovery | Expected Result |
|---------|---------|-----------|---------------|-----------------|
| Chrome | 90+ | Linux | hidraw scan | Full support |
| Chrome | 100+ | Windows | webauthn.dll → HID class | Full support |
| Firefox | 100+ | Linux | hidraw scan | Full support |
| Firefox | 100+ | Windows | Own HID stack (default) or webauthn.dll | Full support |
| Firefox | 100+ (Snap) | Ubuntu | AppArmor blocks hidraw | May fail |
| Edge | 100+ | Windows | webauthn.dll → HID class | Full support |
| Chromium | 90+ | Linux | hidraw scan | Full support |
| Brave | Any | Linux/Windows | Same as Chromium | Full support |
| Chrome | 100+ | macOS | IOKit HID Manager | Full support |
| Firefox | 100+ | macOS | IOKit HID Manager | Full support |
| Safari | 14+ | macOS | Platform authenticator API → IOKit | Full support |

## Security Key Compatibility

| Key | VID:PID | Expected Result |
|-----|---------|-----------------|
| YubiKey 5 NFC | 1050:0407 | Full support (CTAP2 + CTAP1) |
| YubiKey 5C | 1050:0407 | Full support |
| YubiKey Security Key | 1050:0120 | Full support |
| SoloKey v2 | 1209:BEEE | Full support (CTAP2 only) |
| Google Titan | 096E:0858 | Full support |
| Feitian ePass FIDO | 096E:0854 | Full support |
| Nitrokey FIDO2 | 20A0:42B1 | Full support |
| Trezor | 1209:53C1 | May need CTAP1 support |
