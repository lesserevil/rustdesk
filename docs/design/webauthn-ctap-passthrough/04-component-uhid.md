# 04 - Component: Virtual FIDO Device (uhid)

**Assignee**: Developer B
**Estimated effort**: 1-2 weeks
**Dependencies**: Component 03 (protobuf messages)
**New file**: `src/server/ctap_uhid.rs`

## Background

On Linux, `/dev/uhid` allows user-space programs to create virtual HID devices.
Writing a `UHID_CREATE2` event creates a kernel HID device. The kernel's HID
subsystem reads the device's report descriptor and, if it sees the FIDO Alliance
usage page (0xF1D0), creates a `/dev/hidrawN` node that browsers discover as a
FIDO authenticator.

This component creates and manages the virtual FIDO device lifecycle.

## Prerequisites

Before writing code, read:
- Linux kernel docs: https://www.kernel.org/doc/html/latest/hid/uhid.html
- The existing `src/server/uinput.rs` — it creates virtual keyboard/mouse devices
  via a similar kernel interface (`/dev/uinput`). Study its device creation pattern.

## Requirements

### R1: Create a virtual FIDO HID device

Write a function that opens `/dev/uhid`, writes a `UHID_CREATE2` event with
a FIDO-compatible HID report descriptor, and returns a handle for subsequent I/O.

### R2: Read output reports from the browser

When the browser sends a CTAPHID packet to the virtual device, the kernel delivers
it as a `UHID_OUTPUT` event. Read these events from the uhid file descriptor.

### R3: Write input reports to the browser

To send CTAPHID responses back to the browser, write `UHID_INPUT2` events to
the uhid file descriptor.

### R4: Destroy the device on cleanup

Write a `UHID_DESTROY` event when the CTAP service shuts down, and close the
file descriptor.

### R5: Async-compatible I/O

The uhid file descriptor must be usable with `tokio::select!` alongside other
async channels. Use `tokio::io::unix::AsyncFd` to make the fd non-blocking and
pollable.

## Data Structures

### Kernel ABI Constants

```rust
// uhid event types (from linux/uhid.h)
pub const UHID_DESTROY: u32 = 1;
pub const UHID_START: u32 = 2;
pub const UHID_STOP: u32 = 3;
pub const UHID_OPEN: u32 = 4;
pub const UHID_CLOSE: u32 = 5;
pub const UHID_OUTPUT: u32 = 6;
pub const UHID_CREATE2: u32 = 11;
pub const UHID_INPUT2: u32 = 12;

// Bus types
pub const BUS_USB: u16 = 0x03;

// Max data sizes
pub const UHID_DATA_MAX: usize = 4096;
pub const HID_MAX_DESCRIPTOR_SIZE: usize = 4096;
```

### FIDO HID Report Descriptor

This exact byte sequence makes the kernel recognize the device as a FIDO authenticator.
Do not modify it.

```rust
pub const FIDO_HID_REPORT_DESCRIPTOR: [u8; 34] = [
    0x06, 0xD0, 0xF1,  // Usage Page (FIDO Alliance = 0xF1D0)
    0x09, 0x01,         // Usage (U2F Authenticator Device)
    0xA1, 0x01,         // Collection (Application)
    0x09, 0x20,         //   Usage (Input Report Data)
    0x15, 0x00,         //   Logical Minimum (0)
    0x26, 0xFF, 0x00,   //   Logical Maximum (255)
    0x75, 0x08,         //   Report Size (8 bits)
    0x95, 0x40,         //   Report Count (64)
    0x81, 0x02,         //   Input (Data, Variable, Absolute)
    0x09, 0x21,         //   Usage (Output Report Data)
    0x15, 0x00,         //   Logical Minimum (0)
    0x26, 0xFF, 0x00,   //   Logical Maximum (255)
    0x75, 0x08,         //   Report Size (8 bits)
    0x95, 0x40,         //   Report Count (64)
    0x91, 0x02,         //   Output (Data, Variable, Absolute)
    0xC0,               // End Collection
];
```

**Why these bytes matter:**
- `0x06, 0xD0, 0xF1` — Usage Page 0xF1D0 is registered to the FIDO Alliance. Both
  Chrome and Firefox scan HID report descriptors for this value to identify FIDO devices.
- `0x95, 0x40` — Report Count 64, meaning each HID report is 64 bytes. This matches
  the CTAPHID specification.
- No Report ID is defined, so the kernel will NOT prepend a report ID byte to
  reads/writes on the corresponding hidraw device.

### Kernel Struct Layouts

These structs must match the kernel ABI exactly. Use `#[repr(C, packed)]`.

```rust
/// UHID_CREATE2 request — creates a new HID device
#[repr(C, packed)]
pub struct UhidCreate2Req {
    pub name: [u8; 128],                        // Device name (null-terminated UTF-8)
    pub phys: [u8; 64],                         // Physical path (can be empty)
    pub uniq: [u8; 64],                         // Unique identifier (can be empty)
    pub rd_size: u16,                           // Report descriptor size in bytes
    pub bus: u16,                               // Bus type (BUS_USB = 0x03)
    pub vendor: u32,                            // USB Vendor ID
    pub product: u32,                           // USB Product ID
    pub version: u32,                           // Device version
    pub country: u32,                           // HID country code
    pub rd_data: [u8; HID_MAX_DESCRIPTOR_SIZE], // Report descriptor bytes
}

/// UHID_INPUT2 request — send an input report to the host (device → browser)
#[repr(C, packed)]
pub struct UhidInput2Req {
    pub size: u16,                      // Report data size
    pub data: [u8; UHID_DATA_MAX],      // Report data
}

/// UHID_OUTPUT event — received when host writes an output report (browser → device)
#[repr(C, packed)]
pub struct UhidOutputReq {
    pub data: [u8; UHID_DATA_MAX],      // Report data
    pub size: u16,                      // Report data size
    pub rtype: u8,                      // Report type
}

/// Top-level uhid event — all reads/writes use this structure
#[repr(C, packed)]
pub struct UhidEvent {
    pub type_: u32,                     // Event type (UHID_CREATE2, UHID_INPUT2, etc.)
    pub payload: [u8; UHID_EVENT_PAYLOAD_SIZE], // Union of all req types
}
```

**IMPORTANT**: The `UhidEvent` struct in the kernel is a tagged union. The `type_`
field determines which `payload` interpretation is valid. The total struct size is
fixed at `4 + sizeof(largest union member)`. Calculate the payload size as:
`max(sizeof(UhidCreate2Req), sizeof(UhidInput2Req), sizeof(UhidOutputReq))`.

In practice, `UhidCreate2Req` is the largest at `128 + 64 + 64 + 2 + 2 + 4 + 4 + 4 + 4 + 4096 = 4372 bytes`.
So `UHID_EVENT_PAYLOAD_SIZE = 4376` (the kernel header defines the union at 4380
bytes to accommodate alignment; use `std::mem::size_of` on the kernel struct or
just use 4380 to be safe).

## API Design

```rust
/// Handle to a virtual FIDO device created via /dev/uhid.
pub struct VirtualFidoDevice {
    /// The /dev/uhid file descriptor, wrapped for async I/O.
    fd: tokio::io::unix::AsyncFd<std::fs::File>,
}

impl VirtualFidoDevice {
    /// Create a new virtual FIDO device.
    ///
    /// Opens /dev/uhid, writes a UHID_CREATE2 event with the FIDO HID report
    /// descriptor, and returns a handle.
    ///
    /// # Errors
    /// - Permission denied if /dev/uhid is not accessible
    /// - I/O error if the kernel rejects the create request
    pub fn new() -> ResultType<Self>;

    /// Read the next output report from the browser.
    ///
    /// Blocks (async) until the browser sends a CTAPHID packet to the virtual
    /// device. Returns the raw 64-byte HID report.
    ///
    /// Also handles UHID_OPEN/UHID_CLOSE/UHID_START/UHID_STOP events internally
    /// (logs them, does not return them to the caller).
    ///
    /// # Returns
    /// - Ok(report): 64-byte HID output report from the browser
    /// - Err if the device was destroyed or an I/O error occurred
    pub async fn read_output_report(&self) -> ResultType<[u8; 64]>;

    /// Write an input report to the browser.
    ///
    /// Sends a 64-byte CTAPHID packet to the browser via UHID_INPUT2.
    ///
    /// # Arguments
    /// - `report`: Exactly 64 bytes of HID report data
    pub fn write_input_report(&self, report: &[u8; 64]) -> ResultType<()>;

    /// Destroy the virtual device and clean up.
    ///
    /// Sends UHID_DESTROY and closes the file descriptor.
    /// Called automatically on Drop, but can be called explicitly.
    pub fn destroy(&self) -> ResultType<()>;
}

impl Drop for VirtualFidoDevice {
    fn drop(&mut self) {
        let _ = self.destroy();
    }
}
```

## Implementation Guide

### Step 1: Create the file

Create `src/server/ctap_uhid.rs` and add `pub mod ctap_uhid;` to `src/server/mod.rs`
(or wherever server modules are declared — check the existing pattern).

### Step 2: Implement struct creation

```rust
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use hbb_common::{bail, log, ResultType};

impl VirtualFidoDevice {
    pub fn new() -> ResultType<Self> {
        // 1. Open /dev/uhid
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uhid")?;

        // 2. Set non-blocking for async
        let raw_fd = file.as_raw_fd();
        unsafe {
            let flags = libc::fcntl(raw_fd, libc::F_GETFL);
            libc::fcntl(raw_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        // 3. Build UHID_CREATE2 event
        //    Zero-initialize the entire event struct, then fill in fields
        let mut event_bytes = vec![0u8; 4 + UHID_EVENT_PAYLOAD_SIZE];

        // type = UHID_CREATE2 (little-endian u32)
        event_bytes[0..4].copy_from_slice(&UHID_CREATE2.to_ne_bytes());

        // name (offset 4, 128 bytes)
        let name = b"RustDesk Virtual FIDO2 Authenticator";
        event_bytes[4..4 + name.len()].copy_from_slice(name);

        // rd_size (offset 4+128+64+64 = 260, 2 bytes, little-endian)
        let rd_size_offset = 4 + 128 + 64 + 64;
        event_bytes[rd_size_offset..rd_size_offset + 2]
            .copy_from_slice(&(FIDO_HID_REPORT_DESCRIPTOR.len() as u16).to_ne_bytes());

        // bus (offset rd_size_offset+2 = 262, 2 bytes)
        let bus_offset = rd_size_offset + 2;
        event_bytes[bus_offset..bus_offset + 2]
            .copy_from_slice(&BUS_USB.to_ne_bytes());

        // vendor (offset 264, 4 bytes) — use 0x1209 (pid.codes open-source VID)
        let vendor_offset = bus_offset + 2;
        event_bytes[vendor_offset..vendor_offset + 4]
            .copy_from_slice(&0x1209u32.to_ne_bytes());

        // product (offset 268, 4 bytes) — use a unique PID for RustDesk
        let product_offset = vendor_offset + 4;
        event_bytes[product_offset..product_offset + 4]
            .copy_from_slice(&0xF1D0u32.to_ne_bytes());

        // version (offset 272, 4 bytes)
        let version_offset = product_offset + 4;
        event_bytes[version_offset..version_offset + 4]
            .copy_from_slice(&0x0100u32.to_ne_bytes());

        // country = 0 (offset 276, 4 bytes) — already zero

        // rd_data (offset 280, up to 4096 bytes)
        let rd_data_offset = version_offset + 4 + 4; // +4 for country
        event_bytes[rd_data_offset..rd_data_offset + FIDO_HID_REPORT_DESCRIPTOR.len()]
            .copy_from_slice(&FIDO_HID_REPORT_DESCRIPTOR);

        // 4. Write the event
        (&file).write_all(&event_bytes)?;

        log::info!("Created virtual FIDO device via /dev/uhid");

        // 5. Wrap in AsyncFd
        let async_fd = tokio::io::unix::AsyncFd::new(file)?;

        Ok(Self { fd: async_fd })
    }
}
```

**CRITICAL NOTE on struct layout**: The offsets above assume `#[repr(C, packed)]`
layout. The actual kernel struct layout must be verified against your kernel headers.
Write a test (see Testing section) that checks `std::mem::size_of` matches
expectations.

**Alternative approach**: Instead of calculating byte offsets manually, define the
Rust structs with `#[repr(C, packed)]` and use `unsafe` transmutation:

```rust
let mut create_req = UhidCreate2Req {
    name: [0u8; 128],
    phys: [0u8; 64],
    uniq: [0u8; 64],
    rd_size: FIDO_HID_REPORT_DESCRIPTOR.len() as u16,
    bus: BUS_USB,
    vendor: 0x1209,
    product: 0xF1D0,
    version: 0x0100,
    country: 0,
    rd_data: [0u8; HID_MAX_DESCRIPTOR_SIZE],
};
create_req.name[..name.len()].copy_from_slice(name);
create_req.rd_data[..FIDO_HID_REPORT_DESCRIPTOR.len()]
    .copy_from_slice(&FIDO_HID_REPORT_DESCRIPTOR);

// Write as tagged event
let type_bytes = UHID_CREATE2.to_ne_bytes();
file.write_all(&type_bytes)?;
let req_bytes = unsafe {
    std::slice::from_raw_parts(
        &create_req as *const _ as *const u8,
        std::mem::size_of::<UhidCreate2Req>(),
    )
};
file.write_all(req_bytes)?;
```

**Pick one approach and be consistent.** The struct-based approach is cleaner but
requires careful alignment verification.

### Step 3: Implement read_output_report

```rust
impl VirtualFidoDevice {
    pub async fn read_output_report(&self) -> ResultType<[u8; 64]> {
        loop {
            // Wait for the fd to become readable
            let mut guard = self.fd.readable().await?;

            // Try to read a uhid event
            match guard.try_io(|inner| {
                let mut event_buf = vec![0u8; 4 + UHID_EVENT_PAYLOAD_SIZE];
                let n = nix::unistd::read(inner.as_raw_fd(), &mut event_buf)
                    .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
                if n < 4 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Short uhid read",
                    ));
                }
                Ok(event_buf)
            }) {
                Ok(Ok(buf)) => {
                    let event_type = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
                    match event_type {
                        UHID_OUTPUT => {
                            // Extract the output report data
                            // UhidOutputReq layout: data[4096] + size(u16) + rtype(u8)
                            // Data starts at offset 4 (after type field)
                            let size_offset = 4 + UHID_DATA_MAX;
                            let size = u16::from_ne_bytes([
                                buf[size_offset],
                                buf[size_offset + 1],
                            ]) as usize;

                            if size != 64 {
                                log::warn!("Unexpected FIDO HID report size: {}", size);
                                continue;
                            }

                            let mut report = [0u8; 64];
                            report.copy_from_slice(&buf[4..68]);
                            return Ok(report);
                        }
                        UHID_OPEN => {
                            log::info!("uhid: device opened by a process");
                            continue;
                        }
                        UHID_CLOSE => {
                            log::info!("uhid: device closed");
                            continue;
                        }
                        UHID_START => {
                            log::info!("uhid: device started");
                            continue;
                        }
                        UHID_STOP => {
                            log::info!("uhid: device stopped");
                            continue;
                        }
                        other => {
                            log::debug!("uhid: ignoring event type {}", other);
                            continue;
                        }
                    }
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_would_block) => continue, // AsyncFd spurious wake
            }
        }
    }
}
```

### Step 4: Implement write_input_report

```rust
impl VirtualFidoDevice {
    pub fn write_input_report(&self, report: &[u8; 64]) -> ResultType<()> {
        // Build UHID_INPUT2 event
        let mut event_buf = vec![0u8; 4 + 2 + UHID_DATA_MAX];

        // type = UHID_INPUT2
        event_buf[0..4].copy_from_slice(&UHID_INPUT2.to_ne_bytes());

        // size = 64
        event_buf[4..6].copy_from_slice(&64u16.to_ne_bytes());

        // data
        event_buf[6..70].copy_from_slice(report);

        // Write to uhid fd (synchronous write is fine for small data)
        use std::io::Write;
        let fd = self.fd.get_ref();
        (&*fd).write_all(&event_buf)?;

        Ok(())
    }
}
```

### Step 5: Implement destroy

```rust
impl VirtualFidoDevice {
    pub fn destroy(&self) -> ResultType<()> {
        let mut event_buf = vec![0u8; 4 + UHID_EVENT_PAYLOAD_SIZE];
        event_buf[0..4].copy_from_slice(&UHID_DESTROY.to_ne_bytes());

        use std::io::Write;
        let fd = self.fd.get_ref();
        (&*fd).write_all(&event_buf)?;

        log::info!("Destroyed virtual FIDO device");
        Ok(())
    }
}
```

## Testing

### Unit Test: Struct sizes

```rust
#[test]
fn test_uhid_struct_sizes() {
    // Verify our structs match expected kernel ABI sizes
    assert_eq!(std::mem::size_of::<UhidCreate2Req>(), 4372);
    assert_eq!(std::mem::size_of::<UhidInput2Req>(), 4098);
    assert_eq!(std::mem::size_of::<UhidOutputReq>(), 4099);
}
```

### Integration Test: Device creation

This test requires `/dev/uhid` access (run as root or with appropriate permissions):

```rust
#[tokio::test]
#[ignore] // Requires /dev/uhid access
async fn test_create_virtual_fido_device() {
    let device = VirtualFidoDevice::new().expect("Failed to create device");

    // Verify a new /dev/hidraw device appeared
    // (Check /sys/class/hidraw/ for new entries)

    // Cleanup
    device.destroy().expect("Failed to destroy device");
}
```

### Integration Test: Round-trip with hidapi

See [11-testing.md](11-testing.md) for the full integration test that opens the
virtual device from both sides.

## Error Handling

| Error | Cause | Action |
|-------|-------|--------|
| `Permission denied` on `/dev/uhid` | Missing udev rule or not in uhid group | Log error, disable CTAP feature for this session |
| `UHID_CREATE2` write fails | Kernel module not loaded | Log error, suggest `modprobe uhid` |
| Short read from uhid | Kernel bug or fd closed | Return error, service will restart |
| Unexpected report size (!= 64) | Malformed CTAPHID packet | Log warning, skip packet |

## Platform Notes

- This component is **Linux-only**. Gate with `#[cfg(target_os = "linux")]`.
- `/dev/uhid` requires the `uhid` kernel module (loaded by default on most distros).
- The file should be conditionally compiled. In `src/server/mod.rs`:
  ```rust
  #[cfg(target_os = "linux")]
  pub mod ctap_uhid;
  ```

## Acceptance Criteria

- [ ] `VirtualFidoDevice::new()` creates a virtual device visible in `/sys/class/hidraw/`
- [ ] The device's HID report descriptor matches `FIDO_HID_REPORT_DESCRIPTOR`
- [ ] `read_output_report()` returns 64-byte reports when a process writes to the hidraw device
- [ ] `write_input_report()` delivers 64-byte reports to processes reading the hidraw device
- [ ] `destroy()` removes the device from `/sys/class/hidraw/`
- [ ] Drop automatically destroys the device
- [ ] All operations are async-compatible (work inside `tokio::select!`)
- [ ] Struct size tests pass
- [ ] Code compiles only on Linux (`#[cfg(target_os = "linux")]`)
