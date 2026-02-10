//!
//! # Usage from C
//!
//! ```c
//! #include "usbsid_pico.h"   // generated header
//! USBSIDitf us = create_USBSID();
//! init_USBSID(us, true, true);
//! // …
//! close_USBSID(us);
//! ```

use crate::device::UsbSid;

/// Opaque handle used by C callers.
pub type USBSIDitf = *mut UsbSid;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Convert an opaque pointer back to a reference, returning `None` for null.
#[inline]
unsafe fn as_ref<'a>(p: USBSIDitf) -> Option<&'a UsbSid> {
    if p.is_null() {
        None
    } else {
        Some(&*p)
    }
}

/// Convert an opaque pointer back to a mutable reference.
#[inline]
unsafe fn as_mut<'a>(p: USBSIDitf) -> Option<&'a mut UsbSid> {
    if p.is_null() {
        None
    } else {
        Some(&mut *p)
    }
}

// ── Lifecycle ────────────────────────────────────────────────────────────────

/// Allocate a new `UsbSid` on the heap and return an opaque pointer.
#[no_mangle]
pub extern "C" fn create_USBSID() -> USBSIDitf {
    Box::into_raw(Box::new(UsbSid::new()))
}

/// Initialise the driver.  Returns 0 on success, -1 on failure.
///
/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`] and must not be used concurrently.
#[no_mangle]
pub unsafe extern "C" fn init_USBSID(p: USBSIDitf, start_threaded: bool, with_cycles: bool) -> i32 {
    match as_mut(p) {
        Some(us) => match us.init(start_threaded, with_cycles) {
            Ok(()) => 0,
            Err(_) => -1,
        },
        None => -1,
    }
}

/// Close the driver and free the heap allocation.
///
/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`]. Must not be called more than once for the same pointer.
#[no_mangle]
pub unsafe extern "C" fn close_USBSID(p: USBSIDitf) {
    if !p.is_null() {
        let _ = Box::from_raw(p); // Drop runs UsbSid::close via Drop impl
    }
}

// ── SID / device control ─────────────────────────────────────────────────────

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn pause_USBSID(p: USBSIDitf) {
    if let Some(us) = as_mut(p) {
        us.pause();
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn reset_USBSID(p: USBSIDitf) {
    if let Some(us) = as_mut(p) {
        us.reset();
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn resetallregisters_USBSID(p: USBSIDitf) {
    if let Some(us) = as_mut(p) {
        us.reset_all_registers();
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn clearbus_USBSID(p: USBSIDitf) {
    if let Some(us) = as_mut(p) {
        us.clear_bus();
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn mute_USBSID(p: USBSIDitf) {
    if let Some(us) = as_mut(p) {
        us.mute();
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn unmute_USBSID(p: USBSIDitf) {
    if let Some(us) = as_mut(p) {
        us.unmute();
    }
}

// ── Clock rate ───────────────────────────────────────────────────────────────

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn setclockrate_USBSID(
    p: USBSIDitf,
    clockrate_cycles: i64,
    suspend_sids: bool,
) {
    if let Some(us) = as_mut(p) {
        us.set_clock_rate(clockrate_cycles, suspend_sids);
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn getclockrate_USBSID(p: USBSIDitf) -> i64 {
    match as_mut(p) {
        Some(us) => us.get_clock_rate(),
        None => 0,
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn getrefreshrate_USBSID(p: USBSIDitf) -> i64 {
    match as_ref(p) {
        Some(us) => us.get_refresh_rate(),
        None => 0,
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn getrasterrate_USBSID(p: USBSIDitf) -> i64 {
    match as_ref(p) {
        Some(us) => us.get_raster_rate(),
        None => 0,
    }
}

// ── Device info ──────────────────────────────────────────────────────────────

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn getnumsids_USBSID(p: USBSIDitf) -> i32 {
    match as_mut(p) {
        Some(us) => us.get_num_sids(),
        None => -1,
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn getfmoplsid_USBSID(p: USBSIDitf) -> i32 {
    match as_mut(p) {
        Some(us) => us.get_fmopl_sid(),
        None => -1,
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn getpcbversion_USBSID(p: USBSIDitf) -> i32 {
    match as_mut(p) {
        Some(us) => us.get_pcb_version(),
        None => -1,
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn setstereo_USBSID(p: USBSIDitf, state: i32) {
    if let Some(us) = as_mut(p) {
        us.set_stereo(state);
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn togglestereo_USBSID(p: USBSIDitf) {
    if let Some(us) = as_mut(p) {
        us.toggle_stereo();
    }
}

// ── Status helpers ───────────────────────────────────────────────────────────

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn initialised_USBSID(p: USBSIDitf) -> bool {
    as_ref(p).is_some_and(|us| us.is_initialised())
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn available_USBSID(p: USBSIDitf) -> bool {
    as_ref(p).is_some_and(|us| us.is_available())
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn portisopen_USBSID(p: USBSIDitf) -> bool {
    as_ref(p).is_some_and(|us| us.is_open())
}

// ── Synchronous I/O ──────────────────────────────────────────────────────────

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`]. `buff` must point to at least `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn writesingle_USBSID(p: USBSIDitf, buff: *const u8, len: i32) {
    if let Some(us) = as_ref(p) {
        if !buff.is_null() && len > 0 {
            let slice = std::slice::from_raw_parts(buff, len as usize);
            let _ = us.single_write(slice);
        }
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn readsingle_USBSID(p: USBSIDitf, reg: u8) -> u8 {
    match as_ref(p) {
        Some(us) => us.single_read(reg).unwrap_or(0),
        None => 0,
    }
}

// ── Asynchronous direct I/O ──────────────────────────────────────────────────

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`]. `buff` must point to at least `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn writebuffer_USBSID(p: USBSIDitf, buff: *const u8, len: i32) {
    if let Some(us) = as_mut(p) {
        if !buff.is_null() && len > 0 {
            let slice = std::slice::from_raw_parts(buff, len as usize);
            let _ = us.write_buffer(slice);
        }
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn write_USBSID(p: USBSIDitf, reg: u8, val: u8) {
    if let Some(us) = as_mut(p) {
        let _ = us.write(reg, val);
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn writecycled_USBSID(p: USBSIDitf, reg: u8, val: u8, cycles: u16) {
    if let Some(us) = as_mut(p) {
        let _ = us.write_cycled(reg, val, cycles);
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn read_USBSID(p: USBSIDitf, reg: u8) -> u8 {
    match as_mut(p) {
        Some(us) => us.read(reg).unwrap_or(0),
        None => 0,
    }
}

// ── Ring-buffer writes ───────────────────────────────────────────────────────

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn writering_USBSID(p: USBSIDitf, reg: u8, val: u8) {
    if let Some(us) = as_ref(p) {
        let _ = us.write_ring(reg, val);
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn writeringcycled_USBSID(p: USBSIDitf, reg: u8, val: u8, cycles: u16) {
    if let Some(us) = as_ref(p) {
        let _ = us.write_ring_cycled(reg, val, cycles);
    }
}

// ── Thread management ────────────────────────────────────────────────────────

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn enablethread_USBSID(p: USBSIDitf) {
    if let Some(us) = as_mut(p) {
        let _ = us.enable_thread();
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn disablethread_USBSID(p: USBSIDitf) {
    if let Some(us) = as_mut(p) {
        us.disable_thread();
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn setflush_USBSID(p: USBSIDitf) {
    if let Some(us) = as_mut(p) {
        us.set_flush();
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn flush_USBSID(p: USBSIDitf) {
    if let Some(us) = as_mut(p) {
        us.flush();
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn restartringbuffer_USBSID(p: USBSIDitf) {
    if let Some(us) = as_mut(p) {
        us.restart_ring_buffer();
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn setbuffsize_USBSID(p: USBSIDitf, size: i32) {
    if let Some(us) = as_mut(p) {
        us.set_buffer_size(size as usize);
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn setdiffsize_USBSID(p: USBSIDitf, size: i32) {
    if let Some(us) = as_mut(p) {
        us.set_diff_size(size as usize);
    }
}

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn restartthread_USBSID(p: USBSIDitf, with_cycles: bool) {
    if let Some(us) = as_mut(p) {
        let _ = us.restart_thread(with_cycles);
    }
}

// ── Timing ───────────────────────────────────────────────────────────────────

/// # Safety
///
/// `p` must be a valid pointer returned by [`create_USBSID`].
#[no_mangle]
pub unsafe extern "C" fn waitforcycle_USBSID(p: USBSIDitf, cycles: u64) -> i64 {
    match as_mut(p) {
        Some(us) => us.wait_for_cycle(cycles) as i64,
        None => 0,
    }
}
