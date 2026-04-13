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

        // UHID_CREATE2 payload starts at offset 4:
        //   name: [u8; 128] at offset 4
        //   rd_size: u16 at offset 4+128+128+128+2+2+4+4 = offset 400
        //   rd_data: [u8; 4096] at offset 402
        //   bus: u16 at offset 132
        //   vendor: u32 at offset 136
        //   product: u32 at offset 140
        //   Actually, the struct layout for uhid_create2_req is:
        //   u8 name[128];      // offset 4
        //   u16 rd_size;       // offset 132
        //   u16 bus;           // offset 134
        //   u32 vendor;        // offset 136
        //   u32 product;       // offset 140
        //   u32 version;       // offset 144
        //   u32 country;       // offset 148
        //   u8 rd_data[HID_MAX_DESCRIPTOR_SIZE=4096]; // offset 152

        let name = b"RustDesk Virtual FIDO Device";
        let name_offset = 4;
        buf[name_offset..name_offset + name.len()].copy_from_slice(name);

        // rd_size (u16 LE at offset 132)
        let rd_size = FIDO_HID_REPORT_DESCRIPTOR.len() as u16;
        buf[132..134].copy_from_slice(&rd_size.to_le_bytes());

        // bus: BUS_USB = 0x03 (u16 LE at offset 134)
        buf[134..136].copy_from_slice(&3u16.to_le_bytes());

        // vendor (u32 LE at offset 136)
        buf[136..140].copy_from_slice(&(VIRTUAL_FIDO_VID as u32).to_le_bytes());

        // product (u32 LE at offset 140)
        buf[140..144].copy_from_slice(&(VIRTUAL_FIDO_PID as u32).to_le_bytes());

        // version (u32 LE at offset 144)
        buf[144..148].copy_from_slice(&1u32.to_le_bytes());

        // country (u32 LE at offset 148) = 0
        // rd_data starts at offset 152
        let rd_offset = 152;
        buf[rd_offset..rd_offset + FIDO_HID_REPORT_DESCRIPTOR.len()]
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
                // UHID_OUTPUT event layout:
                //   u32 type (offset 0)
                //   u8 data[UHID_DATA_MAX=4096] (offset 4)
                //   u16 size (offset 4+4096 = 4100)
                //   u8 rtype (offset 4102)
                //
                // Actually for uhid_output_req:
                //   u8 data[4096] at offset 4
                //   u16 size at offset 4100
                //   u8 rtype at offset 4102
                let size = u16::from_le_bytes([event_buf[4100], event_buf[4101]]) as usize;
                if size < 64 {
                    log::warn!("uhid output report too short: {} bytes", size);
                    continue;
                }
                let mut report = [0u8; 64];
                report.copy_from_slice(&event_buf[4..68]);
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
