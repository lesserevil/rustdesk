/// Virtual FIDO device trait and platform-specific factory function.
///
/// Each platform implements its own virtual HID device mechanism:
/// - Linux: /dev/uhid
/// - Windows: VHF (Virtual HID Framework) driver (stub)
/// - macOS: DriverKit IOUserHIDDevice (stub)
use hbb_common::ResultType;

/// FIDO HID Report Descriptor.
///
/// Defines a FIDO-compliant HID device with:
/// - Usage Page 0xF1D0 (FIDO Alliance)
/// - Usage 0x01 (U2F/FIDO)
/// - 64-byte input and output reports (no report ID)
pub const FIDO_HID_REPORT_DESCRIPTOR: &[u8] = &[
    0x06, 0xD0, 0xF1, //   Usage Page (FIDO Alliance)
    0x09, 0x01, //   Usage (U2F Authenticator Device)
    0xA1, 0x01, //   Collection (Application)
    0x09, 0x20, //     Usage (Input Report Data)
    0x15, 0x00, //     Logical Minimum (0)
    0x26, 0xFF, 0x00, // Logical Maximum (255)
    0x75, 0x08, //     Report Size (8)
    0x95, 0x40, //     Report Count (64)
    0x81, 0x02, //     Input (Data, Var, Abs)
    0x09, 0x21, //     Usage (Output Report Data)
    0x15, 0x00, //     Logical Minimum (0)
    0x26, 0xFF, 0x00, // Logical Maximum (255)
    0x75, 0x08, //     Report Size (8)
    0x95, 0x40, //     Report Count (64)
    0x91, 0x02, //     Output (Data, Var, Abs)
    0xC0, //   End Collection
];

/// USB vendor/product IDs for the virtual FIDO device.
/// Using pid.codes open-source VID 0x1209 with a RustDesk-specific PID.
pub const VIRTUAL_FIDO_VID: u16 = 0x1209;
pub const VIRTUAL_FIDO_PID: u16 = 0xF1D0;

/// Trait for a virtual FIDO HID device on the remote (server) side.
///
/// The CTAP service uses this trait to interact with the virtual device
/// without knowing the platform-specific implementation.
pub trait VirtualFidoDevice: Send + Sync {
    /// Read an output report (browser → virtual device).
    /// Blocks until a report is available. Returns exactly 64 bytes.
    fn read_output_report(&self) -> ResultType<[u8; 64]>;

    /// Write an input report (virtual device → browser).
    /// Data must be exactly 64 bytes.
    fn write_input_report(&self, data: &[u8; 64]) -> ResultType<()>;

    /// Destroy the virtual device and release all resources.
    fn destroy(&self) -> ResultType<()>;
}

/// Create a platform-appropriate virtual FIDO device.
#[cfg(target_os = "linux")]
pub fn create_virtual_fido_device() -> ResultType<Box<dyn VirtualFidoDevice>> {
    let device = super::ctap_uhid::UhidFidoDevice::create()?;
    Ok(Box::new(device))
}

#[cfg(target_os = "windows")]
pub fn create_virtual_fido_device() -> ResultType<Box<dyn VirtualFidoDevice>> {
    hbb_common::bail!("CTAP virtual device not yet implemented on Windows (VHF driver required)")
}

#[cfg(target_os = "macos")]
pub fn create_virtual_fido_device() -> ResultType<Box<dyn VirtualFidoDevice>> {
    hbb_common::bail!("CTAP virtual device not yet implemented on macOS (DriverKit extension required)")
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub fn create_virtual_fido_device() -> ResultType<Box<dyn VirtualFidoDevice>> {
    hbb_common::bail!("CTAP virtual device not supported on this platform")
}

/// Check if the virtual FIDO device is available on this system.
#[cfg(target_os = "linux")]
pub fn is_virtual_device_available() -> bool {
    use std::path::Path;
    Path::new("/dev/uhid").exists()
}

#[cfg(target_os = "windows")]
pub fn is_virtual_device_available() -> bool {
    super::ctap_vhf::is_driver_available()
}

#[cfg(target_os = "macos")]
pub fn is_virtual_device_available() -> bool {
    super::ctap_driverkit::is_driver_available()
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub fn is_virtual_device_available() -> bool {
    false
}
