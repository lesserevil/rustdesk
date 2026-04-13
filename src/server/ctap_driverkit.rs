/// macOS DriverKit stub.
///
/// Full implementation requires a DriverKit system extension (IOUserHIDDevice)
/// that creates a virtual HID device. This is a separate project requiring
/// Apple Developer ID signing and DriverKit entitlements.

/// Check if the RustDesk DriverKit extension is installed and approved.
pub fn is_driver_available() -> bool {
    // TODO: Check via OSSystemExtensionManager or IOKit
    false
}
