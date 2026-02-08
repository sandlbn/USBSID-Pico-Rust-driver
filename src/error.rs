//! Error types for the USBSID-Pico driver.

use thiserror::Error;

/// All errors that can occur when interacting with a USBSID-Pico device.
#[derive(Debug, Error)]
pub enum UsbSidError {
    /// The driver has not been initialised yet.
    #[error("USBSID driver not initialised")]
    NotInitialised,

    /// The USB port is not open / device not connected.
    #[error("USBSID port is not open")]
    PortNotOpen,

    /// No USBSID-Pico device was found on the bus.
    #[error("USBSID-Pico device not found (VID=0x{vid:04X} PID=0x{pid:04X})")]
    DeviceNotFound { vid: u16, pid: u16 },

    /// A `rusb` (libusb) operation failed.
    #[error("USB error: {0}")]
    Usb(#[from] rusb::Error),

    /// Thread-related error (e.g. failed to spawn).
    #[error("Thread error: {0}")]
    Thread(String),

    /// Attempted to use a direct-write function while the background thread is active.
    #[error("Operation not supported while threaded mode is active")]
    ThreadedModeActive,

    /// Attempted to use a ring-buffer function while not in the correct threaded mode.
    #[error("Ring write requires threaded={expected_threaded}, withcycles={expected_cycled}")]
    WrongThreadMode {
        expected_threaded: bool,
        expected_cycled: bool,
    },
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, UsbSidError>;
