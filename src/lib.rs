//! # usbsid-pico
//! ## Quick start
//!
//! ```rust,no_run
//! use usbsid_pico::UsbSid;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut sid = UsbSid::new();
//!     sid.init(/* threaded */ true, /* with_cycles */ true)?;
//!
//!     // Write register 0x01 with value 0x01
//!     sid.write_ring_cycled(0x01, 0x01, 0xFFFF)?;
//!
//!     // Read a SID register (synchronous)
//!     let val = sid.single_read(0x1B)?;
//!     println!("OSC3 random: 0x{:02X}", val);
//!
//!     // Driver is automatically closed on drop
//!     Ok(())
//! }
//! ```
//!
//! ## C FFI
//!
//! The crate also exposes a C-compatible interface through the [`ffi`] module
//! so that existing C applications can link against this library as a drop-in
//! replacement for the original `USBSIDInterface.h/.cpp`.
//!
//! ## Features
//!
//! * `debug_memory` – enable SID-memory tracking (mirrors `DEBUG_USBSID_MEMORY`
//!   from the C++ driver).

pub mod constants;
pub mod device;
pub mod error;
pub mod ffi;
pub mod ringbuffer;

// Re-export the most commonly used types at crate root.
pub use constants::{sid_address, ClockSpeed, RasterRate, RefreshRate};
pub use device::UsbSid;
pub use error::{Result, UsbSidError};
