# 10 - Component: Configuration & Permissions

**Assignee**: Developer E
**Estimated effort**: 2-3 days
**Dependencies**: Component 03 (protobuf)
**Files to modify**:
- `libs/hbb_common/src/config.rs`
- `src/server/connection.rs` (permission handling — see Component 08)
- System files: udev rules, documentation

## Background

The CTAP passthrough feature requires:
1. A configuration option to enable/disable it (server-side setting)
2. A permission that can be toggled per-connection (like keyboard/clipboard)
3. System-level setup (udev rules for /dev/uhid and /dev/hidraw access)
4. Runtime checks for prerequisite availability

Study the existing permission system:
- Config keys in `libs/hbb_common/src/config.rs` (line ~2796)
- Permission initialization in `src/server/connection.rs` (line ~426)
- The `OPTION_ENABLE_*` pattern used throughout the codebase

## Requirements

### R1: Add config key

In `libs/hbb_common/src/config.rs`, add:

```rust
pub const OPTION_ENABLE_CTAP: &str = "enable-ctap";
```

Add near the other `OPTION_ENABLE_*` constants (around line 2796-2806).

Default value: **disabled** (`"N"` or absent from options map).

### R2: Add permission mapping

In `src/server/connection.rs`, in the `permission()` function (around line 2160),
add the CTAP mapping:

```rust
keys::OPTION_ENABLE_CTAP => Some(Permission::ctap),
```

(Adjust the exact enum variant name to match whatever protobuf-codegen generates
from `Ctap = 8` in the Permission enum.)

### R3: Runtime availability check

Before advertising CTAP support in PeerInfo, check that the feature is actually
usable on this system. The check is platform-specific:

```rust
/// Check if CTAP passthrough is available on this system.
///
/// Requirements:
/// - Linux: /dev/uhid exists and is accessible
/// - Windows: VHF driver is installed and running
/// - macOS: Not supported as remote (server) side
/// - Feature is enabled in config
pub fn is_ctap_available() -> bool {
    // Check feature is enabled (all platforms)
    if !Self::is_permission_enabled_locally(keys::OPTION_ENABLE_CTAP) {
        return false;
    }

    #[cfg(target_os = "linux")]
    {
        use std::path::Path;

        if !Path::new("/dev/uhid").exists() {
            log::debug!("CTAP: /dev/uhid not found");
            return false;
        }

        match std::fs::metadata("/dev/uhid") {
            Ok(meta) => {
                use std::os::unix::fs::MetadataExt;
                let mode = meta.mode();
                if mode & libc::S_IFCHR == 0 {
                    log::debug!("CTAP: /dev/uhid is not a character device");
                    return false;
                }
                true
            }
            Err(e) => {
                log::debug!("CTAP: cannot stat /dev/uhid: {}", e);
                false
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Check that the RustDesk VHF virtual FIDO driver is installed.
        crate::server::ctap_vhf::is_driver_available()
    }

    #[cfg(target_os = "macos")]
    {
        // Check that the RustDesk DriverKit extension is installed and approved.
        crate::server::ctap_driverkit::is_driver_available()
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        false
    }
}
```

### R4: Set ctap_passthrough_supported in PeerInfo

In `send_logon_response_and_keep_alive()` (around line 1481 in connection.rs),
where PeerInfo is built:

```rust
pi.ctap_passthrough_supported = Self::is_ctap_available();
```

### R5: System setup (platform-specific)

#### Linux: Udev rules

**File**: `res/udev/99-rustdesk-ctap.rules`

```udev
# RustDesk CTAP Passthrough — Virtual FIDO Device
# Allows RustDesk to create virtual FIDO authenticators via /dev/uhid

# Grant access to /dev/uhid for the rustdesk group
KERNEL=="uhid", MODE="0660", GROUP="rustdesk"

# Grant access to the virtual FIDO hidraw device created by RustDesk
# VID 1209 (pid.codes open-source), PID F1D0 (RustDesk FIDO)
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="1209", ATTRS{idProduct}=="f1d0", MODE="0664", GROUP="plugdev"
```

**Installation instructions**:

```bash
# Install udev rule
sudo cp res/udev/99-rustdesk-ctap.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules

# Add user to required groups
sudo groupadd -f rustdesk
sudo usermod -aG rustdesk $USER
sudo usermod -aG plugdev $USER

# Load uhid module (if not loaded)
sudo modprobe uhid

# To load uhid automatically on boot:
echo uhid | sudo tee /etc/modules-load.d/uhid.conf
```

#### Windows: VHF driver installation

The RustDesk VHF driver (`rustdesk_vhf.inf` + `rustdesk_vhf.dll`) must be
installed on the remote Windows machine. This can be done:

**Bundled with installer** (recommended):
The RustDesk MSI/NSIS installer includes the signed VHF driver and installs it
via `pnputil` during setup. The installer handles:
- Driver installation: `pnputil /add-driver rustdesk_vhf.inf /install`
- Driver removal on uninstall: `pnputil /delete-driver rustdesk_vhf.inf /uninstall`

**Manual installation**:
```powershell
# Requires Administrator PowerShell
pnputil /add-driver rustdesk_vhf.inf /install

# Verify installation
pnputil /enum-drivers | findstr "rustdesk"

# Remove
pnputil /delete-driver rustdesk_vhf.inf /uninstall /force
```

**Requirements**:
- Windows 10 version 1903 or later (VHF support)
- The driver must be attestation-signed for production (see Component 04, Part B)
- No reboot required — the driver loads on demand when the CTAP service starts

#### macOS: DriverKit system extension

The RustDesk DriverKit extension (`com.rustdesk.RustDeskFIDODriver.dext`) is
embedded in the RustDesk.app bundle. Setup happens on first use:

1. RustDesk detects that CTAP is enabled but the extension is not yet approved
2. RustDesk calls `OSSystemExtensionManager.submitRequest()` to install
3. macOS shows: "RustDesk wants to install a system extension"
4. User approves in System Settings → Privacy & Security → Extensions
5. Extension loads, virtual FIDO device becomes available

The extension persists across reboots — the approval step is one-time only.

**Requirements**:
- macOS 10.15 (Catalina) or later
- User must approve the system extension in System Settings
- RustDesk.app must be signed with Developer ID and notarized
- DriverKit entitlements must be approved by Apple

### R6: Client-side availability check

The client also needs a runtime check — is there a physical FIDO key connected?
This works on all platforms (Linux, Windows, macOS) via `hidapi`.

```rust
/// Check if a physical FIDO authenticator is available on this machine.
///
/// This is a quick enumeration — it does NOT open any device.
/// Works on Linux (hidraw), Windows (Windows HID API), and macOS (IOHidManager).
pub fn is_local_fido_available() -> bool {
    match hidapi::HidApi::new() {
        Ok(api) => {
            api.device_list()
                .any(|d| d.usage_page() == 0xF1D0 && d.usage() == 0x01)
        }
        Err(e) => {
            log::debug!("hidapi initialization failed: {}", e);
            false
        }
    }
}
```

This is called by the native client when deciding whether to send
`CtapControl{enabled: true}` after login. The web client performs an equivalent
check by connecting to the companion app and reading the `fido_available` field
from the `hello_ack` response.

## Configuration Flow

```mermaid
flowchart TD
    A[Server starts] --> B{OPTION_ENABLE_CTAP == Y?}
    B -->|No| C[ctap_passthrough_supported = false in PeerInfo]
    B -->|Yes| D{Platform check}
    D -->|"Linux: /dev/uhid accessible?"| E{Yes/No}
    D -->|"Windows: VHF driver installed?"| E
    D -->|"macOS: DriverKit extension approved?"| E
    E -->|No| C
    E -->|Yes| F[ctap_passthrough_supported = true in PeerInfo]

    F --> G[Client receives PeerInfo]
    G --> H{Client type?}
    H -->|Native| I{hidapi finds FIDO key?}
    H -->|Web| J{Companion app connected + FIDO key?}
    I -->|No| K[Do not send CtapControl]
    J -->|No| K
    I -->|Yes| L[Send CtapControl enabled=true]
    J -->|Yes| L

    L --> M[Server spawns CTAP service]
```

## Settings UI

The CTAP toggle should appear in:

### Server settings (remote machine)

In the RustDesk settings UI, under "Security" or "Permissions":

```
[x] Allow keyboard input
[x] Allow clipboard sync
[x] Allow file transfer
[ ] Allow security key passthrough    <-- NEW
[x] Allow audio
```

This maps to `OPTION_ENABLE_CTAP` in the config.

### Client settings (local machine)

In the connection toolbar / session settings:

```
Permissions:
  [x] Keyboard
  [x] Clipboard
  [ ] Security Key    <-- NEW (only shown if server supports it)
```

This sends `CtapControl` messages and/or permission switch IPC.

## Testing

### Test: Feature disabled by default

```rust
#[test]
fn test_ctap_disabled_by_default() {
    // Fresh config should not have OPTION_ENABLE_CTAP set
    let config = Config2::default();
    assert_ne!(
        config.options.get(keys::OPTION_ENABLE_CTAP),
        Some(&"Y".to_string())
    );
}
```

### Test: PeerInfo reflects availability

```rust
#[test]
fn test_peer_info_ctap_support() {
    // When CTAP is enabled and uhid is available, PeerInfo should report support
    // (This is an integration test that depends on system state)
}
```

### Manual Test: Udev rules

1. Install the udev rule
2. Add user to `rustdesk` group
3. Log out and back in
4. Verify: `ls -la /dev/uhid` shows group `rustdesk` with mode `0660`
5. Verify: `test -w /dev/uhid && echo "writable"` succeeds

## Acceptance Criteria

- [ ] `OPTION_ENABLE_CTAP` config key exists
- [ ] Feature is disabled by default
- [ ] `Permission::Ctap` is wired into the permission system
- [ ] PeerInfo includes `ctap_passthrough_supported` based on runtime check
- [ ] Runtime check verifies: Linux, /dev/uhid exists, feature enabled in config
- [ ] Client-side check verifies physical FIDO key is connected
- [ ] Udev rule file is created and documented
- [ ] Settings UI shows CTAP toggle on both server and client sides
- [ ] Toggling the setting off stops the CTAP service if running
