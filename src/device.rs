// usbsid-pico – Rust driver for the USBSID-Pico USB SID interface.
//
// Licensed under MIT OR Apache-2.0

//! Core driver struct and implementation.
//!
//! # Architecture
//!
//! The driver communicates with a USBSID-Pico device over USB bulk transfers.
//! It supports three write modes:
//!
//! 1. **Synchronous** – blocking `libusb_bulk_transfer` calls (`single_write` / `single_read`).
//! 2. **Asynchronous direct** – non-blocking via `libusb_submit_transfer` (the `write*` family).
//! 3. **Asynchronous threaded** – writes go into a ring buffer; a background thread drains
//!    it and submits USB transfers (the `write_ring*` family).
//!

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
#[cfg(target_os = "macos")]
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use log::{debug, error, warn};

use crate::constants::*;
use crate::error::{Result, UsbSidError};
use crate::ringbuffer::RingBuffer;
use crate::transport::Transport;
// ── Internal shared state ────────────────────────────────────────────────────

/// State shared between the main thread and the background writer thread.
/// The ring buffer is NOT in here — it's lock-free and accessed directly
/// via `Arc<RingBuffer>` without the mutex (SPSC safe).
struct SharedState {
    flush: bool,
    /// Set to `true` by the writer thread after a flush has been fully drained.
    flush_done: bool,
}

// ── Main driver ──────────────────────────────────────────────────────────────

/// Handle to a single USBSID-Pico device.
///
/// Create with [`UsbSid::new`] and then call [`UsbSid::init`] to open the
/// connection and optionally start the background writer thread.
pub struct UsbSid {
    // ── Transport abstraction (single connection, shared with writer thread) ─
    transport: Option<Arc<Mutex<Box<dyn Transport>>>>,

    // ── Buffers ────────────────────────────────────────────────────────────
    out_buffer: Vec<u8>,
    write_buffer: Vec<u8>,
    thread_buffer: Vec<u8>,
    #[allow(dead_code)]
    result_buffer: Vec<u8>,
    len_out_buffer: usize,
    buffer_pos: usize,

    // ── Driver state ─────────────────────────────────────────────────────
    initialised: bool,
    port_is_open: bool,

    // ── Threading ────────────────────────────────────────────────────────
    threaded: bool,
    with_cycles: bool,
    run_thread: Arc<AtomicBool>,
    /// Lock-free SPSC ring buffer — accessed directly by producer and consumer
    /// without the mutex, matching the C++ driver's RingPut/RingGet pattern.
    ring: Arc<RingBuffer>,
    shared: Arc<Mutex<SharedState>>,
    /// Notifies the writer thread when data is available or flush is requested,
    /// and notifies the main thread when a flush completes.
    cond: Arc<Condvar>,
    thread_handle: Option<JoinHandle<()>>,
    thread_count: Arc<AtomicI32>,

    // ── Clock / timing ───────────────────────────────────────────────────
    cycles_per_sec: i64,
    cycles_per_frame: i64,
    cycles_per_raster: i64,
    cpu_cycle_duration_ns: f64,
    last_time: Instant,
    clk_retrieved: bool,
    us_clkrate: i64,

    // ── Device info (cached) ─────────────────────────────────────────────
    num_sids: i32,
    fmopl_sid: i32,
    pcb_version: i32,
    socket_config_retrieved: bool,

    // ── Ring-buffer sizes ────────────────────────────────────────────────
    ring_size: usize,
    diff_size: usize,
}

impl UsbSid {
    // ═════════════════════════════════════════════════════════════════════
    //  Construction
    // ═════════════════════════════════════════════════════════════════════

    /// Create a new, uninitialised driver instance.
    ///
    /// You **must** call [`init`](Self::init) before any USB I/O.
    pub fn new() -> Self {
        debug!("[USBSID] Driver init start");
        Self {
            transport: None,

            out_buffer: vec![0u8; LEN_OUT_BUFFER],
            write_buffer: vec![0u8; LEN_OUT_BUFFER],
            thread_buffer: vec![0u8; LEN_OUT_BUFFER],
            result_buffer: vec![0u8; LEN_IN_BUFFER],
            len_out_buffer: LEN_OUT_BUFFER,
            buffer_pos: 1,

            initialised: true,
            port_is_open: false,

            threaded: false,
            with_cycles: false,
            run_thread: Arc::new(AtomicBool::new(false)),
            ring: Arc::new(RingBuffer::with_defaults()),
            shared: Arc::new(Mutex::new(SharedState {
                flush: false,
                flush_done: false,
            })),
            cond: Arc::new(Condvar::new()),
            thread_handle: None,
            thread_count: Arc::new(AtomicI32::new(0)),

            cycles_per_sec: ClockSpeed::Default as i64,
            cycles_per_frame: RefreshRate::Default as i64,
            cycles_per_raster: RasterRate::Default as i64,
            cpu_cycle_duration_ns: 1_000_000_000.0 / (ClockSpeed::Default as i64 as f64),
            last_time: Instant::now(),
            clk_retrieved: false,
            us_clkrate: 0,

            num_sids: 0,
            fmopl_sid: -1,
            pcb_version: -1,
            socket_config_retrieved: false,

            ring_size: DEFAULT_RING_SIZE,
            diff_size: DEFAULT_DIFF_SIZE,
        }
    }

    // ═════════════════════════════════════════════════════════════════════
    //  Lifecycle
    // ═════════════════════════════════════════════════════════════════════

    /// Open the USB connection and optionally start the background writer thread.
    ///
    /// * `start_threaded` – if `true`, spawns a thread that drains the ring buffer.
    /// * `with_cycles` – if `true`, each ring-buffer entry carries a 16-bit cycle count.
    ///
    /// Returns `Ok(())` on success.
    pub fn init(&mut self, start_threaded: bool, with_cycles: bool) -> Result<()> {
        if !self.initialised {
            return Err(UsbSidError::NotInitialised);
        }
        if self.port_is_open {
            debug!("[USBSID] Driver already started");
            return Ok(());
        }

        debug!("[USBSID] Setup start");
        self.threaded = start_threaded;
        self.with_cycles = with_cycles;

        // Try available backends. USB first (if enabled), then serial.
        #[cfg(feature = "usb")]
        match self.libusb_setup() {
            Ok(()) => {
                debug!("[USBSID] USB (libusb) backend connected");
                if self.threaded {
                    self.init_thread()?;
                }
                let _ = self.get_clock_rate();
                self.port_is_open = true;
                return Ok(());
            }
            Err(e) => {
                debug!("[USBSID] USB failed: {e}");
            }
        }

        #[cfg(feature = "serial")]
        match self.serial_setup() {
            Ok(()) => {
                debug!("[USBSID] Serial backend connected");
                if self.threaded {
                    self.init_thread()?;
                }
                let _ = self.get_clock_rate();
                self.port_is_open = true;
                return Ok(());
            }
            Err(e) => {
                debug!("[USBSID] Serial failed: {e}");
            }
        }

        Err(UsbSidError::DeviceNotFound {
            vid: VENDOR_ID,
            pid: PRODUCT_ID,
        })
    }

    /// Close the USB connection, stop the writer thread, and release all resources.
    pub fn close(&mut self) {
        if !self.port_is_open {
            return;
        }
        self.stop_thread();
        self.transport = None;
        self.port_is_open = false;
        debug!("[USBSID] Closed device");
    }

    // ═════════════════════════════════════════════════════════════════════
    //  Status queries
    // ═════════════════════════════════════════════════════════════════════

    /// `true` once the constructor has run (always `true` for Rust callers).
    pub fn is_initialised(&self) -> bool {
        self.initialised
    }

    /// `true` if a USBSID-Pico was detected on the bus during [`init`](Self::init).
    pub fn is_available(&self) -> bool {
        self.transport.is_some()
    }

    /// `true` if the USB port is currently open and ready for I/O.
    pub fn is_open(&self) -> bool {
        self.port_is_open
    }

    // ═════════════════════════════════════════════════════════════════════
    //  SID / device control commands
    // ═════════════════════════════════════════════════════════════════════

    /// Pause playback (releases chip-select pins on device).
    pub fn pause(&mut self) {
        if !self.port_is_open {
            return;
        }
        debug!("[USBSID] Pause");
        let buf = [command_byte(CMD_PAUSE), 0, 0];
        let _ = self.single_write(&buf);
    }

    /// Reset all SID chips.
    pub fn reset(&mut self) {
        if !self.port_is_open {
            return;
        }
        debug!("[USBSID] Reset");
        let buf = [command_byte(CMD_RESET_SID), 0, 0];
        let _ = self.single_write(&buf);
        if let Ok(mut s) = self.shared.lock() {
            s.flush = true;
        }
        self.sync_time();
    }

    /// Reset every register on all SID chips.
    pub fn reset_all_registers(&mut self) {
        if !self.port_is_open {
            return;
        }
        debug!("[USBSID] Reset All Registers");
        let buf = [command_byte(CMD_RESET_SID), 0x01, 0];
        let _ = self.single_write(&buf);
    }

    /// Mute all SID chips.
    pub fn mute(&mut self) {
        if !self.port_is_open {
            return;
        }
        debug!("[USBSID] Mute");
        let buf = [command_byte(CMD_MUTE), 0, 0];
        let _ = self.single_write(&buf);
    }

    /// Un-mute all SID chips.
    pub fn unmute(&mut self) {
        if !self.port_is_open {
            return;
        }
        debug!("[USBSID] UnMute");
        let buf = [command_byte(CMD_UNMUTE), 0, 0];
        let _ = self.single_write(&buf);
    }

    /// Disable SID (release reset pin and unmute).
    pub fn disable_sid(&mut self) {
        if !self.port_is_open {
            return;
        }
        debug!("[USBSID] DisableSID");
        let buf = [command_byte(CMD_DISABLE_SID), 0, 0];
        let _ = self.single_write(&buf);
    }

    /// Enable SID (assert reset pin and release chip-select).
    pub fn enable_sid(&mut self) {
        if !self.port_is_open {
            return;
        }
        debug!("[USBSID] EnableSID");
        let buf = [command_byte(CMD_ENABLE_SID), 0, 0];
        let _ = self.single_write(&buf);
    }

    /// Clear the SID data bus.
    pub fn clear_bus(&mut self) {
        if !self.port_is_open {
            return;
        }
        debug!("[USBSID] ClearBus");
        let buf = [command_byte(CMD_CLEAR_BUS), 0, 0];
        let _ = self.single_write(&buf);
    }

    // ═════════════════════════════════════════════════════════════════════
    //  Clock rate
    // ═════════════════════════════════════════════════════════════════════

    /// Set the CPU clock rate.
    ///
    /// `clockrate` must match one of the [`ClockSpeed`] variants.
    /// If `suspend_sids` is `true` the device will assert the SID RES signal
    /// while changing the clock (recommended).
    pub fn set_clock_rate(&mut self, clockrate: i64, suspend_sids: bool) {
        if !self.port_is_open {
            return;
        }
        for (i, &(speed, refresh, raster)) in CLOCK_TABLE.iter().enumerate() {
            if speed as i64 == clockrate {
                self.cycles_per_sec = speed as i64;
                self.cycles_per_frame = refresh as i64;
                self.cycles_per_raster = raster as i64;
                self.cpu_cycle_duration_ns = 1_000_000_000.0 / (self.cycles_per_sec as f64);

                debug!("[USBSID] Clockspeed set to: {}", self.cycles_per_sec);

                if !self.clk_retrieved || self.us_clkrate != self.cycles_per_sec {
                    let suspend_byte = if suspend_sids { 1u8 } else { 0u8 };
                    let buf = [command_byte(CMD_CONFIG), 0x50, i as u8, suspend_byte, 0, 0];
                    let _ = self.single_write(&buf);
                }
                self.sync_time();
                return;
            }
        }
    }

    /// Retrieve the current CPU clock rate from the device (cached after first call).
    pub fn get_clock_rate(&mut self) -> i64 {
        if !self.port_is_open {
            return 0;
        }
        if self.clk_retrieved {
            self.cpu_cycle_duration_ns = 1_000_000_000.0 / (self.cycles_per_sec as f64);
            return self.cycles_per_sec;
        }

        self.sync_time();
        let buf = [command_byte(CMD_CONFIG), 0x57, 0, 0, 0, 0];
        let _ = self.single_write(&buf);
        let idx = self.single_read_config(1) as usize;
        if idx < CLOCK_TABLE.len() {
            self.us_clkrate = CLOCK_TABLE[idx].0 as i64;
            self.cycles_per_sec = self.us_clkrate;
            self.cycles_per_frame = CLOCK_TABLE[idx].1 as i64;
            self.cycles_per_raster = CLOCK_TABLE[idx].2 as i64;
        }
        self.clk_retrieved = true;
        self.cpu_cycle_duration_ns = 1_000_000_000.0 / (self.cycles_per_sec as f64);
        self.cycles_per_sec
    }

    /// Get cycles per video refresh frame.
    pub fn get_refresh_rate(&self) -> i64 {
        self.cycles_per_frame
    }

    /// Get cycles per raster line frame.
    pub fn get_raster_rate(&self) -> i64 {
        self.cycles_per_raster
    }

    // ═════════════════════════════════════════════════════════════════════
    //  Device info queries
    // ═════════════════════════════════════════════════════════════════════

    /// Get the total number of SID chips configured on the device.
    pub fn get_num_sids(&mut self) -> i32 {
        if !self.port_is_open {
            return 0;
        }
        if self.num_sids != 0 {
            return self.num_sids;
        }
        let buf = [command_byte(CMD_CONFIG), 0x39, 0, 0, 0, 0];
        let _ = self.single_write(&buf);
        self.num_sids = self.single_read_config(1) as i32;
        self.num_sids
    }

    /// Get the SID number configured for FM-OPL (returns -1 if not configured).
    pub fn get_fmopl_sid(&mut self) -> i32 {
        if !self.port_is_open {
            return 0;
        }
        if self.fmopl_sid == -1 {
            let buf = [command_byte(CMD_CONFIG), 0x3A, 0, 0, 0, 0];
            let _ = self.single_write(&buf);
            self.fmopl_sid = self.single_read_config(1) as i32;
        }
        if self.fmopl_sid == 0 {
            -1
        } else {
            self.fmopl_sid
        }
    }

    /// Get the PCB hardware version of the device.
    pub fn get_pcb_version(&mut self) -> i32 {
        if !self.port_is_open {
            return 0;
        }
        if self.pcb_version == -1 {
            let buf = [command_byte(CMD_CONFIG), 0x81, 0x01, 0, 0, 0];
            let _ = self.single_write(&buf);
            self.pcb_version = self.single_read_config(1) as i32;
        }
        self.pcb_version
    }

    /// Set the device to mono (0) or stereo (1) – v1.3 PCB only.
    pub fn set_stereo(&mut self, state: i32) {
        if !self.port_is_open {
            return;
        }
        if self.pcb_version == -1 {
            self.get_pcb_version();
        }
        if self.pcb_version == 13 {
            let buf = [command_byte(CMD_CONFIG), 0x89, state as u8, 0, 0, 0];
            let _ = self.single_write(&buf);
        }
    }

    /// Toggle between mono and stereo – v1.3 PCB only.
    pub fn toggle_stereo(&mut self) {
        if !self.port_is_open {
            return;
        }
        if self.pcb_version == -1 {
            self.get_pcb_version();
        }
        if self.pcb_version == 13 {
            let buf = [command_byte(CMD_CONFIG), 0x88, 0, 0, 0, 0];
            let _ = self.single_write(&buf);
        }
    }

    /// Retrieve the raw 10-byte socket configuration from the device.
    ///
    /// Returns `None` if the response is malformed or the device is not connected.
    pub fn get_socket_config(&mut self) -> Option<[u8; 10]> {
        if !self.port_is_open {
            return None;
        }
        if self.socket_config_retrieved {
            return None;
        }

        let buf = [command_byte(CMD_CONFIG), 0x37, 0, 0, 0, 0];
        let _ = self.single_write(&buf);

        let mut config = [0u8; 10];
        if let Ok(10) = self.recv_bytes(&mut config) {
            if config[0] == 0x37 && config[1] == 0x7F && config[9] == 0xFF {
                self.socket_config_retrieved = true;
                return Some(config);
            }
        }
        None
    }

    // ═════════════════════════════════════════════════════════════════════
    //  Synchronous I/O (blocking bulk transfers)
    // ═════════════════════════════════════════════════════════════════════

    /// Blocking write of an arbitrary buffer to the device.
    pub fn single_write(&self, buf: &[u8]) -> Result<()> {
        if !self.port_is_open {
            return Err(UsbSidError::PortNotOpen);
        }
        self.send_bytes(buf)
    }

    /// Blocking read of a single SID register.
    pub fn single_read(&self, reg: u8) -> Result<u8> {
        if !self.port_is_open {
            return Err(UsbSidError::PortNotOpen);
        }

        // Send read command
        let cmd = [read_header(), reg, 0];
        self.send_bytes(&cmd)?;

        // Receive result
        let mut result = [0u8; 1];
        match self.recv_bytes(&mut result) {
            Ok(_) => Ok(result[0]),
            Err(e) => {
                warn!("[USBSID] Error reading register 0x{:02X}: {e}", reg);
                Ok(0)
            }
        }
    }

    /// Internal helper: send a config command and read back `len` bytes, returning the first byte.
    fn single_read_config(&self, len: usize) -> u8 {
        if !self.port_is_open {
            return 0;
        }
        let mut buf = vec![0u8; len];
        match self.recv_bytes(&mut buf) {
            Ok(_) => buf[0],
            Err(e) => {
                error!("[USBSID] Error reading config: {}", e);
                0
            }
        }
    }

    // ═════════════════════════════════════════════════════════════════════
    //  Asynchronous direct writes (non-threaded mode)
    // ═════════════════════════════════════════════════════════════════════

    /// Write a raw buffer to the device (async, non-threaded only).
    ///
    /// This performs a synchronous bulk transfer under the hood (matching
    /// Simplified async write using synchronous bulk transfers.
    /// since we let `rusb` manage the transfer lifecycle).
    pub fn write_buffer(&mut self, buf: &[u8]) -> Result<()> {
        if !self.port_is_open {
            return Err(UsbSidError::PortNotOpen);
        }
        if self.threaded {
            return Err(UsbSidError::ThreadedModeActive);
        }
        self.send_bytes(buf)
    }

    /// Write a single register + value pair (async, non-threaded).
    pub fn write(&mut self, reg: u8, val: u8) -> Result<()> {
        if !self.port_is_open {
            return Err(UsbSidError::PortNotOpen);
        }
        if self.threaded {
            return Err(UsbSidError::ThreadedModeActive);
        }
        let buf = [0x00, reg, val];
        self.write_buffer(&buf)
    }

    /// Write a register + value pair with an associated cycle delay that the
    /// device will use to pace the write.
    pub fn write_cycled(&mut self, reg: u8, val: u8, cycles: u16) -> Result<()> {
        if !self.port_is_open {
            return Err(UsbSidError::PortNotOpen);
        }
        if self.threaded {
            return Err(UsbSidError::ThreadedModeActive);
        }
        let buf = [
            cycled_write_header(0),
            reg,
            val,
            (cycles >> 8) as u8,
            (cycles & 0xFF) as u8,
        ];
        self.write_buffer(&buf)
    }

    /// Asynchronous read of a SID register (non-threaded only).
    pub fn read(&mut self, reg: u8) -> Result<u8> {
        if !self.port_is_open {
            return Err(UsbSidError::PortNotOpen);
        }
        if self.threaded {
            return Err(UsbSidError::ThreadedModeActive);
        }

        let cmd = [read_header(), reg];
        self.send_bytes(&cmd)?;
        let mut result = [0u8; 1];
        self.recv_bytes(&mut result)?;
        Ok(result[0])
    }

    // ═════════════════════════════════════════════════════════════════════
    //  Ring-buffer writes (threaded mode)
    // ═════════════════════════════════════════════════════════════════════

    /// Push a register + value pair into the ring buffer (non-cycled threaded mode).
    /// Lock-free — pushes directly to the SPSC ring without acquiring the mutex.
    pub fn write_ring(&self, reg: u8, val: u8) -> Result<()> {
        if !self.port_is_open {
            return Err(UsbSidError::PortNotOpen);
        }
        if !self.threaded || self.with_cycles {
            return Err(UsbSidError::WrongThreadMode {
                expected_threaded: true,
                expected_cycled: false,
            });
        }
        if self.ring.is_full() {
            warn!("[USBSID] Ring buffer overflow (non-cycled write dropped)");
            return Ok(());
        }
        self.ring.put(reg);
        self.ring.put(val);
        self.cond.notify_one();
        Ok(())
    }

    /// Push a register + value + 16-bit cycle count into the ring buffer (cycled threaded mode).
    /// Lock-free — pushes directly to the SPSC ring without acquiring the mutex.
    pub fn write_ring_cycled(&self, reg: u8, val: u8, cycles: u16) -> Result<()> {
        if !self.port_is_open {
            return Err(UsbSidError::PortNotOpen);
        }
        if !(self.threaded && self.with_cycles) {
            return Err(UsbSidError::WrongThreadMode {
                expected_threaded: true,
                expected_cycled: true,
            });
        }
        if self.ring.is_full() {
            warn!("[USBSID] Ring buffer overflow (cycled write dropped)");
            return Ok(());
        }
        self.ring.put(reg);
        self.ring.put(val);
        self.ring.put((cycles >> 8) as u8);
        self.ring.put((cycles & 0xFF) as u8);
        self.cond.notify_one();
        Ok(())
    }

    /// Push multiple register + value + 16-bit cycle count entries into the
    /// ring buffer (cycled threaded mode).
    /// Lock-free — pushes directly to the SPSC ring without acquiring the mutex.
    /// Returns the number of writes successfully pushed.
    pub fn write_ring_cycled_batch(&self, writes: &[(u8, u8, u16)]) -> Result<usize> {
        if !self.port_is_open {
            return Err(UsbSidError::PortNotOpen);
        }
        if !(self.threaded && self.with_cycles) {
            return Err(UsbSidError::WrongThreadMode {
                expected_threaded: true,
                expected_cycled: true,
            });
        }
        let mut pushed = 0usize;
        for &(reg, val, cycles) in writes {
            if self.ring.is_full() {
                eprintln!(
                    "[USBSID] Ring buffer overflow ({} of {} cycled writes dropped, capacity={})",
                    writes.len() - pushed,
                    writes.len(),
                    self.ring.capacity(),
                );
                break;
            }
            self.ring.put(reg);
            self.ring.put(val);
            self.ring.put((cycles >> 8) as u8);
            self.ring.put((cycles & 0xFF) as u8);
            pushed += 1;
        }
        if pushed > 0 {
            self.cond.notify_one();
        }
        Ok(pushed)
    }

    // ═════════════════════════════════════════════════════════════════════
    //  Thread management
    // ═════════════════════════════════════════════════════════════════════

    /// Enable the background writer thread on the fly.
    pub fn enable_thread(&mut self) -> Result<()> {
        if !self.port_is_open {
            return Err(UsbSidError::PortNotOpen);
        }
        if !self.run_thread.load(Ordering::SeqCst) {
            self.init_thread()?;
        }
        Ok(())
    }

    /// Disable the background writer thread.
    pub fn disable_thread(&mut self) {
        if !self.port_is_open {
            return;
        }
        self.stop_thread();
    }

    /// Signal that the ring-buffer should be flushed.
    pub fn set_flush(&mut self) {
        self.sync_time();
        if let Ok(mut s) = self.shared.lock() {
            s.flush = true;
            s.flush_done = false;
        }
        self.cond.notify_one();
    }

    /// Signal a flush and block until the writer thread has drained the ring.
    pub fn flush(&mut self) {
        if !self.port_is_open {
            return;
        }
        self.set_flush();

        // Wait for the writer thread to signal completion (up to 100ms).
        if let Ok(locked) = self.shared.lock() {
            let _r = self
                .cond
                .wait_timeout_while(locked, Duration::from_millis(100), |s| !s.flush_done);
        }
    }

    /// Restart the ring buffer (deinit + reinit).
    pub fn restart_ring_buffer(&mut self) {
        if !self.port_is_open {
            return;
        }
        if let Some(r) = Arc::get_mut(&mut self.ring) {
            r.reinit_defaults();
        }
    }

    /// Set the ring-buffer capacity.
    pub fn set_buffer_size(&mut self, size: usize) {
        self.ring_size = size;
        if let Some(r) = Arc::get_mut(&mut self.ring) {
            r.set_ring_size(size);
        }
    }

    /// Set the ring-buffer diff threshold.
    pub fn set_diff_size(&mut self, size: usize) {
        self.diff_size = size;
        if let Some(r) = Arc::get_mut(&mut self.ring) {
            r.set_diff_size(size);
        }
    }

    /// Stop the current thread, re-initialise buffers, and restart.
    pub fn restart_thread(&mut self, with_cycles: bool) -> Result<()> {
        if !self.port_is_open {
            return Err(UsbSidError::PortNotOpen);
        }
        self.stop_thread();
        self.threaded = true;
        self.with_cycles = with_cycles;
        self.len_out_buffer = LEN_OUT_BUFFER;
        self.out_buffer = vec![0u8; LEN_OUT_BUFFER];
        self.write_buffer = vec![0u8; LEN_OUT_BUFFER];
        self.thread_buffer = vec![0u8; LEN_OUT_BUFFER];
        self.init_thread()
    }

    // ═════════════════════════════════════════════════════════════════════
    //  Timing & cycles
    // ═════════════════════════════════════════════════════════════════════

    /// Busy-wait for `cycles` CPU cycles at the currently configured clock rate.
    ///
    /// Returns the actual elapsed time in nanoseconds.
    pub fn wait_for_cycle(&mut self, cycles: u64) -> u64 {
        let duration_ns = (cycles as f64 * self.cpu_cycle_duration_ns) as u64;
        let start = Instant::now();
        let target = start + Duration::from_nanos(duration_ns);

        while Instant::now() < target {
            // spin
        }

        start.elapsed().as_nanos() as u64
    }

    /// Reset the internal timestamp used by cycle-delay functions.
    pub fn sync_time(&mut self) {
        self.last_time = Instant::now();
    }

    // ═════════════════════════════════════════════════════════════════════
    //  Private: I/O dispatch (single transport, shared with writer thread)
    // ═════════════════════════════════════════════════════════════════════

    /// Send bytes to the device via the shared transport.
    fn send_bytes(&self, data: &[u8]) -> Result<()> {
        if let Some(ref t) = self.transport {
            let mut guard = t.lock().unwrap();
            guard.send(data)?;
            return Ok(());
        }
        Err(UsbSidError::PortNotOpen)
    }

    /// Receive bytes from the device via the shared transport.
    fn recv_bytes(&self, buf: &mut [u8]) -> Result<usize> {
        if let Some(ref t) = self.transport {
            let mut guard = t.lock().unwrap();
            return guard.recv(buf);
        }
        Err(UsbSidError::PortNotOpen)
    }

    // ═════════════════════════════════════════════════════════════════════
    //  Private: USB setup (creates transport, stores in self.transport)
    // ═════════════════════════════════════════════════════════════════════

    #[cfg(feature = "usb")]
    fn libusb_setup(&mut self) -> Result<()> {
        use crate::transport::usb::UsbTransport;

        debug!("[USBSID] Attempting USB (libusb) connection");
        let transport = UsbTransport::open()?;
        self.transport = Some(Arc::new(Mutex::new(
            Box::new(transport) as Box<dyn Transport>
        )));
        Ok(())
    }

    // ── Private: serial port setup ──────────────────────────────────────

    #[cfg(feature = "serial")]
    fn serial_setup(&mut self) -> Result<()> {
        use crate::transport::serial::SerialTransport;

        debug!("[USBSID] Attempting serial port connection");
        let transport = SerialTransport::open_auto()?;
        self.transport = Some(Arc::new(Mutex::new(
            Box::new(transport) as Box<dyn Transport>
        )));
        debug!("[USBSID] Serial port connected");
        Ok(())
    }

    // ═════════════════════════════════════════════════════════════════════
    //  Private: threading (single shared transport, no dual handle)
    // ═════════════════════════════════════════════════════════════════════

    fn init_thread(&mut self) -> Result<()> {
        debug!("[USBSID] Init Thread start");
        self.buffer_pos = 1;
        self.threaded = true;
        self.with_cycles = true;

        // Re-initialise the ring buffer (lock-free, but reinit needs &mut)
        // Safe: thread is not running yet.
        Arc::get_mut(&mut self.ring)
            .expect("ring has no other refs before thread start")
            .reinit(self.ring_size, self.diff_size);
        {
            let mut s = self.shared.lock().unwrap();
            s.flush = false;
        }

        self.run_thread.store(true, Ordering::SeqCst);
        self.thread_count.fetch_add(1, Ordering::SeqCst);

        // Clone the Arc to the existing transport – no second handle opened.
        let transport = self.transport.clone().ok_or(UsbSidError::PortNotOpen)?;

        let run_flag = Arc::clone(&self.run_thread);
        let ring = Arc::clone(&self.ring);
        let shared = Arc::clone(&self.shared);
        let cond = Arc::clone(&self.cond);
        let with_cycles = self.with_cycles;
        let thread_count = Arc::clone(&self.thread_count);
        let len_out = self.len_out_buffer;

        let join = thread::Builder::new()
            .name("USBSID Thread".into())
            .spawn(move || {
                Self::writer_thread(
                    run_flag,
                    ring,
                    shared,
                    cond,
                    with_cycles,
                    transport,
                    len_out,
                );
                thread_count.fetch_sub(1, Ordering::SeqCst);
            })
            .map_err(|e| UsbSidError::Thread(e.to_string()))?;

        self.thread_handle = Some(join);
        Ok(())
    }

    fn stop_thread(&mut self) {
        if !self.run_thread.load(Ordering::SeqCst) {
            return;
        }
        debug!("[USBSID] Stop thread");
        self.run_thread.store(false, Ordering::SeqCst);
        self.cond.notify_all(); // Wake writer thread so it sees run=false

        // Wait for the thread to finish
        if let Some(h) = self.thread_handle.take() {
            let _ = h.join();
        }

        // Wait for thread_count to reach 0
        while self.thread_count.load(Ordering::SeqCst) > 0 {
            thread::yield_now();
        }

        self.threaded = false;
        self.with_cycles = false;

        // Reset ring buffer (thread is stopped, so we have exclusive access).
        if let Some(r) = Arc::get_mut(&mut self.ring) {
            r.reinit_defaults();
        }
    }

    /// The background writer thread.
    ///
    /// On macOS, spawns a dedicated USB I/O thread to absorb the ~18ms
    /// bulk transfer latency of the CDC-ACM driver.  Ring drains happen
    /// instantly; USB sends run in parallel via a channel.
    ///
    /// On Linux/Windows, uses the original direct-send path (USB bulk
    /// transfers complete in sub-millisecond, no pipelining needed).
    fn writer_thread(
        run: Arc<AtomicBool>,
        ring: Arc<RingBuffer>,
        shared: Arc<Mutex<SharedState>>,
        cond: Arc<Condvar>,
        with_cycles: bool,
        transport: Arc<Mutex<Box<dyn Transport>>>,
        len_out: usize,
    ) {
        #[cfg(target_os = "macos")]
        Self::writer_thread_macos(run, ring, shared, cond, with_cycles, transport, len_out);

        #[cfg(not(target_os = "macos"))]
        Self::writer_thread_default(run, ring, shared, cond, with_cycles, transport, len_out);
    }

    /// macOS writer: async USB I/O thread + mega-send (64-byte aligned packets).
    #[cfg(target_os = "macos")]
    fn writer_thread_macos(
        run: Arc<AtomicBool>,
        ring: Arc<RingBuffer>,
        shared: Arc<Mutex<SharedState>>,
        cond: Arc<Condvar>,
        with_cycles: bool,
        transport: Arc<Mutex<Box<dyn Transport>>>,
        _len_out: usize,
    ) {
        debug!("[USBSID] Writer thread starting (macOS async path)");

        // Use a regular (unbounded) channel so the writer thread never blocks
        // waiting for the USB I/O thread to finish.  The C++ driver uses async
        // libusb transfers which are non-blocking — this matches that behavior.
        let (usb_tx, usb_rx) = mpsc::channel::<Vec<u8>>();
        let io_transport = Arc::clone(&transport);
        let io_run = Arc::clone(&run);

        let io_thread = thread::Builder::new()
            .name("USBSID USB-IO".into())
            .spawn(move || {
                while let Ok(buf) = usb_rx.recv() {
                    if !io_run.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Ok(mut t) = io_transport.lock() {
                        if let Err(e) = t.send(&buf) {
                            eprintln!("[USBSID-IO] USB send failed: {e}, retrying");
                            let _ = t.send(&buf);
                        }
                    }
                }
            })
            .ok();

        let mut buffer_pos: usize = 1;
        let mut thread_buf = vec![0u8; 64];

        while run.load(Ordering::SeqCst) {
            // Check flush flag (brief lock) and ring state (lock-free).
            let flushing = {
                let mut locked = shared.lock().unwrap();
                if !locked.flush && !ring.has_data() {
                    if ring.is_empty() {
                        locked = cond
                            .wait_timeout(locked, Duration::from_millis(1))
                            .unwrap()
                            .0;
                    } else {
                        locked = cond
                            .wait_timeout(locked, Duration::from_micros(100))
                            .unwrap()
                            .0;
                    }
                }
                let f = locked.flush;
                if f {
                    locked.flush = false;
                }
                f
            }; // mutex released here

            if flushing {
                let ring_pending = ring.diff();
                let max_packets = (ring_pending / 4).max(1) + 2;
                let mut mega_buf: Vec<u8> = Vec::with_capacity(max_packets * 64);
                let mut pkt = [0u8; 64];
                let mut pkt_pos: usize = 1;

                if with_cycles {
                    if buffer_pos >= 5 {
                        thread_buf[0] = cycled_write_header((buffer_pos - 1) as u8);
                        pkt[..buffer_pos].copy_from_slice(&thread_buf[..buffer_pos]);
                        mega_buf.extend_from_slice(&pkt[..64]);
                        pkt.fill(0);
                        thread_buf.fill(0);
                    }
                } else if buffer_pos >= 3 {
                    thread_buf[0] = write_header((buffer_pos - 1) as u8);
                    pkt[..buffer_pos].copy_from_slice(&thread_buf[..buffer_pos]);
                    mega_buf.extend_from_slice(&pkt[..64]);
                    pkt.fill(0);
                    thread_buf.fill(0);
                }

                // Drain ring lock-free — no mutex needed.
                while !ring.is_empty() {
                    if with_cycles {
                        pkt[pkt_pos] = ring.get();
                        pkt[pkt_pos + 1] = ring.get();
                        pkt[pkt_pos + 2] = ring.get();
                        pkt[pkt_pos + 3] = ring.get();
                        pkt_pos += 4;
                        if pkt_pos >= 61 || ring.is_empty() {
                            pkt[0] = cycled_write_header((pkt_pos - 1) as u8);
                            mega_buf.extend_from_slice(&pkt[..64]);
                            pkt.fill(0);
                            pkt_pos = 1;
                        }
                    } else {
                        pkt[pkt_pos] = ring.get();
                        pkt[pkt_pos + 1] = ring.get();
                        pkt_pos += 2;
                        if pkt_pos >= 63 || ring.is_empty() {
                            pkt[0] = write_header((pkt_pos - 1) as u8);
                            mega_buf.extend_from_slice(&pkt[..64]);
                            pkt.fill(0);
                            pkt_pos = 1;
                        }
                    }
                }

                if !mega_buf.is_empty() {
                    let _ = usb_tx.send(mega_buf);
                }
                buffer_pos = 1;

                // Signal flush completion.
                let mut locked = shared.lock().unwrap();
                locked.flush_done = true;
                cond.notify_all();
            } else {
                // Normal (non-flush) drain — lock-free ring access.
                while ring.has_data() {
                    if with_cycles {
                        thread_buf[buffer_pos] = ring.get();
                        buffer_pos += 1;
                        thread_buf[buffer_pos] = ring.get();
                        buffer_pos += 1;
                        thread_buf[buffer_pos] = ring.get();
                        buffer_pos += 1;
                        thread_buf[buffer_pos] = ring.get();
                        buffer_pos += 1;
                        if buffer_pos >= 61 {
                            thread_buf[0] = cycled_write_header((buffer_pos - 1) as u8);
                            let mut pkt = [0u8; 64];
                            pkt[..buffer_pos].copy_from_slice(&thread_buf[..buffer_pos]);
                            let _ = usb_tx.send(pkt.to_vec());
                            thread_buf.fill(0);
                            buffer_pos = 1;
                        }
                    } else {
                        thread_buf[buffer_pos] = ring.get();
                        buffer_pos += 1;
                        thread_buf[buffer_pos] = ring.get();
                        buffer_pos += 1;
                        if buffer_pos >= 63 {
                            thread_buf[0] = write_header((buffer_pos - 1) as u8);
                            let mut pkt = [0u8; 64];
                            pkt[..buffer_pos].copy_from_slice(&thread_buf[..buffer_pos]);
                            let _ = usb_tx.send(pkt.to_vec());
                            thread_buf.fill(0);
                            buffer_pos = 1;
                        }
                    }
                }
            }
        }

        drop(usb_tx);
        if let Some(h) = io_thread {
            let _ = h.join();
        }
    }

    /// Linux/Windows writer: direct-send path with lock-free ring access.
    #[cfg(not(target_os = "macos"))]
    fn writer_thread_default(
        run: Arc<AtomicBool>,
        ring: Arc<RingBuffer>,
        shared: Arc<Mutex<SharedState>>,
        cond: Arc<Condvar>,
        with_cycles: bool,
        transport: Arc<Mutex<Box<dyn Transport>>>,
        len_out: usize,
    ) {
        debug!("[USBSID] Thread starting");
        let mut thread_buf = vec![0u8; len_out];
        let mut out_buf = vec![0u8; len_out];
        let mut buffer_pos: usize = 1;

        while run.load(Ordering::SeqCst) {
            // Check flush flag (brief lock) and ring state (lock-free).
            let flushing = {
                let mut locked = shared.lock().unwrap();
                if !locked.flush && !ring.has_data() {
                    if ring.is_empty() {
                        locked = cond
                            .wait_timeout(locked, Duration::from_millis(1))
                            .unwrap()
                            .0;
                    } else {
                        locked = cond
                            .wait_timeout(locked, Duration::from_micros(100))
                            .unwrap()
                            .0;
                    }
                }
                let f = locked.flush;
                if f {
                    locked.flush = false;
                }
                f
            }; // mutex released here

            if flushing {
                // Pre-flush: send any partial packet from thread_buf.
                let min_pos = if with_cycles { 5 } else { 3 };
                if buffer_pos >= min_pos {
                    thread_buf[0] = if with_cycles {
                        cycled_write_header((buffer_pos - 1) as u8)
                    } else {
                        write_header((buffer_pos - 1) as u8)
                    };
                    out_buf[..buffer_pos].copy_from_slice(&thread_buf[..buffer_pos]);
                    let send_len = buffer_pos;
                    buffer_pos = 1;
                    thread_buf.fill(0);
                    if let Ok(mut t) = transport.lock() {
                        if let Err(e) = t.send(&out_buf[..send_len]) {
                            warn!("[USBSID] USB send failed (pre-flush): {e}, retrying");
                            let _ = t.send(&out_buf[..send_len]);
                        }
                    }
                    out_buf.fill(0);
                }
            }

            // Drain ring — lock-free, no mutex needed.
            while if flushing {
                !ring.is_empty()
            } else {
                ring.has_data()
            } {
                if with_cycles {
                    thread_buf[buffer_pos] = ring.get();
                    buffer_pos += 1;
                    thread_buf[buffer_pos] = ring.get();
                    buffer_pos += 1;
                    thread_buf[buffer_pos] = ring.get();
                    buffer_pos += 1;
                    thread_buf[buffer_pos] = ring.get();
                    buffer_pos += 1;

                    if buffer_pos >= 61 || buffer_pos >= len_out {
                        thread_buf[0] = cycled_write_header((buffer_pos - 1) as u8);
                        out_buf[..buffer_pos].copy_from_slice(&thread_buf[..buffer_pos]);
                        let send_len = buffer_pos;
                        buffer_pos = 1;
                        if let Ok(mut t) = transport.lock() {
                            if let Err(e) = t.send(&out_buf[..send_len]) {
                                warn!("[USBSID] USB send failed (cycled drain): {e}, retrying");
                                let _ = t.send(&out_buf[..send_len]);
                            }
                        }
                        thread_buf.fill(0);
                        out_buf.fill(0);
                    }
                } else {
                    thread_buf[buffer_pos] = ring.get();
                    buffer_pos += 1;
                    thread_buf[buffer_pos] = ring.get();
                    buffer_pos += 1;

                    if buffer_pos >= 63 || buffer_pos >= len_out {
                        thread_buf[0] = write_header((buffer_pos - 1) as u8);
                        out_buf[..buffer_pos].copy_from_slice(&thread_buf[..buffer_pos]);
                        let send_len = buffer_pos;
                        buffer_pos = 1;
                        if let Ok(mut t) = transport.lock() {
                            if let Err(e) = t.send(&out_buf[..send_len]) {
                                warn!("[USBSID] USB send failed (drain): {e}, retrying");
                                let _ = t.send(&out_buf[..send_len]);
                            }
                        }
                        thread_buf.fill(0);
                        out_buf.fill(0);
                    }
                }
            }

            if flushing {
                // Post-flush: send any remaining partial packet.
                let min_pos = if with_cycles { 5 } else { 3 };
                if buffer_pos >= min_pos {
                    thread_buf[0] = if with_cycles {
                        cycled_write_header((buffer_pos - 1) as u8)
                    } else {
                        write_header((buffer_pos - 1) as u8)
                    };
                    out_buf[..buffer_pos].copy_from_slice(&thread_buf[..buffer_pos]);
                    let send_len = buffer_pos;
                    buffer_pos = 1;
                    thread_buf.fill(0);
                    if let Ok(mut t) = transport.lock() {
                        if let Err(e) = t.send(&out_buf[..send_len]) {
                            warn!("[USBSID] USB send failed (post-flush): {e}, retrying");
                            let _ = t.send(&out_buf[..send_len]);
                        }
                    }
                    out_buf.fill(0);
                }

                // Signal flush completion.
                let mut locked = shared.lock().unwrap();
                locked.flush_done = true;
                cond.notify_all();
            }
        }

        debug!("[USBSID] Thread finished");
    }
}

impl Default for UsbSid {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for UsbSid {
    fn drop(&mut self) {
        self.close();
    }
}
