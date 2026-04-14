/// Linux uhid-based virtual FIDO device implementation.
///
/// Creates a virtual HID device via /dev/uhid that appears as a FIDO2
/// authenticator to the browser. The browser writes CTAPHID output reports
/// to the device, and we inject input reports (responses) back.
use super::ctap_virtual_device::*;
use hbb_common::{bail, log, ResultType};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::sync::Mutex;

// uhid ioctl constants (from linux/uhid.h)
const UHID_CREATE2: u32 = 11;
const UHID_DESTROY: u32 = 1;
const UHID_INPUT2: u32 = 12;
const UHID_OUTPUT: u32 = 6;

/// Size of uhid_event struct in the kernel.
/// This is a union of various event types; we use the largest common size.
const UHID_EVENT_SIZE: usize = 4380;

pub struct UhidFidoDevice {
    file: Mutex<File>,
}

impl UhidFidoDevice {
    /// Create a new virtual FIDO device via /dev/uhid.
    pub fn create() -> ResultType<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uhid")?;

        let device = Self {
            file: Mutex::new(file),
        };
        device.send_create()?;
        log::info!("Created virtual FIDO device via uhid");
        Ok(device)
    }

    fn send_create(&self) -> ResultType<()> {
        let mut buf = vec![0u8; UHID_EVENT_SIZE];

        // Event type: UHID_CREATE2 (u32 LE at offset 0)
        buf[0..4].copy_from_slice(&(UHID_CREATE2 as u32).to_le_bytes());

        // uhid_create2_req layout (packed, union starts at offset 4):
        //   u8 name[128]   offset 4
        //   u8 phys[64]    offset 132
        //   u8 uniq[64]    offset 196
        //   u16 rd_size    offset 260
        //   u16 bus        offset 262
        //   u32 vendor     offset 264
        //   u32 product    offset 268
        //   u32 version    offset 272
        //   u32 country    offset 276
        //   u8 rd_data[4096] offset 280
        const BASE: usize = 4;
        const RD_SIZE_OFF: usize = BASE + 128 + 64 + 64; // 260
        const BUS_OFF: usize = RD_SIZE_OFF + 2;           // 262
        const VENDOR_OFF: usize = BUS_OFF + 2;            // 264
        const PRODUCT_OFF: usize = VENDOR_OFF + 4;        // 268
        const VERSION_OFF: usize = PRODUCT_OFF + 4;       // 272
        const COUNTRY_OFF: usize = VERSION_OFF + 4;       // 276
        const RD_DATA_OFF: usize = COUNTRY_OFF + 4;       // 280

        let name = b"RustDesk Virtual FIDO Device";
        buf[BASE..BASE + name.len()].copy_from_slice(name);

        let rd_size = FIDO_HID_REPORT_DESCRIPTOR.len() as u16;
        buf[RD_SIZE_OFF..RD_SIZE_OFF + 2].copy_from_slice(&rd_size.to_le_bytes());
        buf[BUS_OFF..BUS_OFF + 2].copy_from_slice(&3u16.to_le_bytes()); // BUS_USB
        buf[VENDOR_OFF..VENDOR_OFF + 4].copy_from_slice(&(VIRTUAL_FIDO_VID as u32).to_le_bytes());
        buf[PRODUCT_OFF..PRODUCT_OFF + 4].copy_from_slice(&(VIRTUAL_FIDO_PID as u32).to_le_bytes());
        buf[VERSION_OFF..VERSION_OFF + 4].copy_from_slice(&1u32.to_le_bytes());
        // country at COUNTRY_OFF = 0 (already zeroed)
        buf[RD_DATA_OFF..RD_DATA_OFF + FIDO_HID_REPORT_DESCRIPTOR.len()]
            .copy_from_slice(FIDO_HID_REPORT_DESCRIPTOR);

        let mut file = self.file.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        file.write_all(&buf)?;
        Ok(())
    }

    fn send_destroy_event(&self) -> ResultType<()> {
        let mut buf = vec![0u8; UHID_EVENT_SIZE];
        buf[0..4].copy_from_slice(&(UHID_DESTROY as u32).to_le_bytes());
        let mut file = self.file.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        file.write_all(&buf)?;
        Ok(())
    }
}

impl VirtualFidoDevice for UhidFidoDevice {
    fn read_output_report(&self) -> ResultType<[u8; 64]> {
        let mut event_buf = vec![0u8; UHID_EVENT_SIZE];
        loop {
            let mut file = self.file.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
            let n = file.read(&mut event_buf)?;
            if n < 4 {
                continue;
            }

            let event_type = u32::from_le_bytes([
                event_buf[0],
                event_buf[1],
                event_buf[2],
                event_buf[3],
            ]);

            if event_type == UHID_OUTPUT {
                // UHID_OUTPUT event: u8 data[4096] at offset 4, u16 size at 4100
                // For FIDO devices without report IDs, the kernel prepends a 0x00
                // report ID byte, making the actual HID report start at data[1].
                let size = u16::from_le_bytes([event_buf[4100], event_buf[4101]]) as usize;
                let mut report = [0u8; 64];
                if size == 65 && event_buf[4] == 0x00 {
                    // Report ID 0x00 prepended — skip it
                    report.copy_from_slice(&event_buf[5..69]);
                } else if size >= 64 {
                    report.copy_from_slice(&event_buf[4..68]);
                } else {
                    log::warn!("uhid output report too short: {} bytes", size);
                    continue;
                }
                return Ok(report);
            }
            // Ignore other event types (UHID_START, UHID_STOP, UHID_OPEN, UHID_CLOSE)
        }
    }

    fn write_input_report(&self, data: &[u8; 64]) -> ResultType<()> {
        let mut buf = vec![0u8; UHID_EVENT_SIZE];

        // Event type: UHID_INPUT2
        buf[0..4].copy_from_slice(&(UHID_INPUT2 as u32).to_le_bytes());

        // UHID_INPUT2 layout:
        //   u16 size at offset 4
        //   u8 data[4096] at offset 6
        let size = 64u16;
        buf[4..6].copy_from_slice(&size.to_le_bytes());
        buf[6..70].copy_from_slice(data);

        let mut file = self.file.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        file.write_all(&buf)?;
        Ok(())
    }

    fn destroy(&self) -> ResultType<()> {
        self.send_destroy_event()?;
        log::info!("Destroyed virtual FIDO device via uhid");
        Ok(())
    }
}

impl Drop for UhidFidoDevice {
    fn drop(&mut self) {
        if let Err(e) = self.send_destroy_event() {
            log::warn!("Failed to destroy uhid device on drop: {}", e);
        }
    }
}
