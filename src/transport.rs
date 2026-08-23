// usbsid-pico – Rust driver for the USBSID-Pico USB SID interface.
//
// Licensed under MIT OR Apache-2.0

//! Transport backends for communicating with the USBSID-Pico.
//!
//! On Linux and macOS the driver uses `rusb` (libusb) to talk directly to the
//! USB bulk endpoints. On Windows (or when the `serial` feature is enabled)
//! the driver can instead open the device as a serial (COM) port, which avoids
//! the need for WinUSB driver installation.

#![allow(unused_imports)]

use std::time::Duration;

use crate::constants::*;
use crate::error::{Result, UsbSidError};

// ─────────────────────────────────────────────────────────────────────────────
//  Transport trait
// ─────────────────────────────────────────────────────────────────────────────

/// Abstraction over the USB transport layer.
///
/// Both the libusb and serial-port backends implement this trait so that
/// [`crate::device::UsbSid`] can be transport-agnostic.
pub(crate) trait Transport: Send {
    /// Send a packet to the device. Returns bytes written.
    fn send(&mut self, data: &[u8]) -> Result<usize>;

    /// Receive bytes from the device. Returns bytes read.
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize>;

    /// Receive bytes with an explicit timeout in milliseconds. The default
    /// implementation falls back to `recv`, which uses the transport's
    /// hard-coded 1 s timeout. Backends that can do better (e.g. libusb's
    /// `read_bulk`) should override.
    fn recv_timeout(&mut self, buf: &mut [u8], _timeout_ms: u32) -> Result<usize> {
        self.recv(buf)
    }

    /// Clear a halted IN endpoint. Default is a no-op — only the libusb
    /// backend can actually issue a USB CLEAR_HALT.
    fn clear_in_halt(&mut self) {}
}

// ─────────────────────────────────────────────────────────────────────────────
//  libusb backend (Linux, macOS, or explicit)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "usb")]
pub(crate) mod usb {
    use super::*;
    use rusb::{Context, DeviceHandle, UsbContext};

    /// libusb-backed transport.
    pub struct UsbTransport {
        pub handle: DeviceHandle<Context>,
        #[allow(dead_code)]
        ctx: Context, // kept alive to prevent libusb context from being dropped
    }

    impl UsbTransport {
        /// Open and configure the USBSID-Pico via libusb.
        pub fn open() -> Result<Self> {
            let ctx = Context::new().map_err(UsbSidError::Usb)?;

            // Debug-only enumeration dump. RUST_LOG=usbsid_pico=debug to see.
            match ctx.devices() {
                Ok(list) => {
                    log::debug!("libusb sees {} device(s)", list.len());
                    for dev in list.iter() {
                        if let Ok(d) = dev.device_descriptor() {
                            log::debug!(
                                "  bus={:03} addr={:03}  VID={:04X} PID={:04X}",
                                dev.bus_number(),
                                dev.address(),
                                d.vendor_id(),
                                d.product_id()
                            );
                        }
                    }
                }
                Err(e) => log::debug!("libusb device enumeration failed: {e}"),
            }

            // Iterate devices manually instead of `open_device_with_vid_pid`,
            // which swallows the open() error and returns None — making
            // permission/busy errors indistinguishable from "not present".
            let devices = ctx.devices().map_err(UsbSidError::Usb)?;
            let mut handle: Option<DeviceHandle<Context>> = None;
            let mut last_open_err: Option<rusb::Error> = None;
            for dev in devices.iter() {
                let desc = match dev.device_descriptor() {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if desc.vendor_id() != VENDOR_ID || desc.product_id() != PRODUCT_ID {
                    continue;
                }
                match dev.open() {
                    Ok(h) => {
                        handle = Some(h);
                        break;
                    }
                    Err(e) => last_open_err = Some(e),
                }
            }
            let handle = match handle {
                Some(h) => h,
                None => {
                    return Err(match last_open_err {
                        Some(e) => UsbSidError::Usb(e),
                        None => UsbSidError::DeviceNotFound {
                            vid: VENDOR_ID,
                            pid: PRODUCT_ID,
                        },
                    });
                }
            };

            // macOS: claim the vendor interface (4) because AppleUSBCDC
            // exclusively owns the CDC interfaces (0/1). The control
            // transfer is a WebUSB-style "connect" handshake — the
            // firmware's tud_vendor_rx_cb drops every byte until
            // web_serial_connected is true.
            #[cfg(target_os = "macos")]
            {
                handle.claim_interface(4)?;
                let _ = handle.set_alternate_setting(4, 0);
                handle.write_control(0x21, 0x22, 0x01, 4, &[], Duration::from_secs(1))?;
            }

            #[cfg(not(target_os = "macos"))]
            {
                for if_num in 0..2u8 {
                    #[cfg(target_os = "linux")]
                    {
                        if handle.kernel_driver_active(if_num).unwrap_or(false) {
                            let _ = handle.detach_kernel_driver(if_num);
                        }
                    }
                    handle.claim_interface(if_num)?;
                }
                // CDC SET_CONTROL_LINE_STATE (DTR | RTS)
                handle.write_control(
                    0x21,
                    0x22,
                    ACM_CTRL_DTR | ACM_CTRL_RTS,
                    0,
                    &[],
                    Duration::from_secs(1),
                )?;
                // CDC SET_LINE_CODING (baud ignored by TinyUSB, required by spec)
                handle.write_control(0x21, 0x20, 0, 0, &LINE_ENCODING, Duration::from_secs(1))?;
            }

            Ok(Self { handle, ctx })
        }
    }

    impl Transport for UsbTransport {
        fn send(&mut self, data: &[u8]) -> Result<usize> {
            self.handle
                .write_bulk(EP_OUT_ADDR, data, Duration::from_secs(1))
                .map_err(UsbSidError::Usb)
        }

        fn recv(&mut self, buf: &mut [u8]) -> Result<usize> {
            self.handle
                .read_bulk(EP_IN_ADDR, buf, Duration::from_secs(1))
                .map_err(UsbSidError::Usb)
        }

        fn recv_timeout(&mut self, buf: &mut [u8], timeout_ms: u32) -> Result<usize> {
            self.handle
                .read_bulk(EP_IN_ADDR, buf, Duration::from_millis(timeout_ms as u64))
                .map_err(UsbSidError::Usb)
        }

        fn clear_in_halt(&mut self) {
            let _ = self.handle.clear_halt(EP_IN_ADDR);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Serial port backend (Windows default, optional elsewhere)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "serial")]
pub(crate) mod serial {
    use super::*;
    use serialport::SerialPort;

    const BAUD_RATE: u32 = 9_000_000;
    const TIMEOUT: Duration = Duration::from_secs(1);

    /// Serial-port-backed transport.
    pub struct SerialTransport {
        port: Box<dyn SerialPort>,
    }

    impl SerialTransport {
        /// Open by explicit port name (e.g. "COM3", "/dev/ttyACM0").
        pub fn open(port_name: &str) -> Result<Self> {
            let port = serialport::new(port_name, BAUD_RATE)
                .timeout(TIMEOUT)
                .data_bits(serialport::DataBits::Eight)
                .parity(serialport::Parity::None)
                .stop_bits(serialport::StopBits::One)
                .flow_control(serialport::FlowControl::None)
                .open()
                .map_err(|e| UsbSidError::Thread(format!("Serial open failed: {e}")))?;

            Ok(Self { port })
        }

        /// Auto-detect USBSID-Pico by scanning available ports for VID:PID.
        pub fn open_auto() -> Result<Self> {
            let ports = serialport::available_ports()
                .map_err(|e| UsbSidError::Thread(format!("Port enumeration failed: {e}")))?;

            for port in &ports {
                if let serialport::SerialPortType::UsbPort(info) = &port.port_type {
                    if info.vid == VENDOR_ID && info.pid == PRODUCT_ID {
                        log::debug!(
                            "[USBSID] Found device on {} (VID={:04X} PID={:04X})",
                            port.port_name,
                            info.vid,
                            info.pid
                        );
                        return Self::open(&port.port_name);
                    }
                }
            }

            Err(UsbSidError::DeviceNotFound {
                vid: VENDOR_ID,
                pid: PRODUCT_ID,
            })
        }

        /// Clone the underlying port for use in a writer thread.
        #[allow(dead_code)]
        pub fn try_clone(&self) -> Result<Self> {
            let port = self
                .port
                .try_clone()
                .map_err(|e| UsbSidError::Thread(format!("Serial clone failed: {e}")))?;
            Ok(Self { port })
        }
    }

    impl Transport for SerialTransport {
        fn send(&mut self, data: &[u8]) -> Result<usize> {
            use std::io::Write;
            self.port
                .write(data)
                .map_err(|e| UsbSidError::Thread(format!("Serial write failed: {e}")))
        }

        fn recv(&mut self, buf: &mut [u8]) -> Result<usize> {
            use std::io::Read;
            self.port
                .read(buf)
                .map_err(|e| UsbSidError::Thread(format!("Serial read failed: {e}")))
        }
    }
}
