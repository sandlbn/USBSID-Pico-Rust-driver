//! Constants, protocol opcodes, and clock/timing definitions ported from `USBSID.h`.

// ── USB identifiers ──────────────────────────────────────────────────────────

/// USB Vendor ID for USBSID-Pico.
pub const VENDOR_ID: u16 = 0xCAFE;
/// USB Product ID for USBSID-Pico.
pub const PRODUCT_ID: u16 = 0x4011;

// ── CDC-ACM control lines ────────────────────────────────────────────────────

pub const ACM_CTRL_DTR: u16 = 0x01;
pub const ACM_CTRL_RTS: u16 = 0x02;

// ── Endpoint addresses ───────────────────────────────────────────────────────
// Linux/Windows use the CDC-ACM interfaces; macOS uses the vendor interface
// because `AppleUSBCDC` exclusively claims the CDC ones.

#[cfg(not(target_os = "macos"))]
pub const EP_OUT_ADDR: u8 = 0x02;
#[cfg(not(target_os = "macos"))]
pub const EP_IN_ADDR: u8 = 0x82;

#[cfg(target_os = "macos")]
pub const EP_OUT_ADDR: u8 = 0x04;
#[cfg(target_os = "macos")]
pub const EP_IN_ADDR: u8 = 0x84;

// ── Buffer sizes ─────────────────────────────────────────────────────────────

/// Size of the IN (read) buffer in bytes.
pub const LEN_IN_BUFFER: usize = 1;
/// Size of the OUT (write) buffer in bytes.
pub const LEN_OUT_BUFFER: usize = 64;

// ── Protocol opcodes (byte 0, top 2 bits) ────────────────────────────────────

/// Plain register write (top 2 bits = 0b00).
pub const OP_WRITE: u8 = 0;
/// Register read request (top 2 bits = 0b01).
pub const OP_READ: u8 = 1;
/// Cycle-delayed write (top 2 bits = 0b10).
pub const OP_CYCLED_WRITE: u8 = 2;
/// Device command (top 2 bits = 0b11).
pub const OP_COMMAND: u8 = 3;

// ── Command sub-opcodes (byte 0, lower 6 bits when top 2 = COMMAND) ──────────

pub const CMD_PAUSE: u8 = 10;
pub const CMD_UNPAUSE: u8 = 11;
pub const CMD_MUTE: u8 = 12;
pub const CMD_UNMUTE: u8 = 13;
pub const CMD_RESET_SID: u8 = 14;
pub const CMD_DISABLE_SID: u8 = 15;
pub const CMD_ENABLE_SID: u8 = 16;
pub const CMD_CLEAR_BUS: u8 = 17;
pub const CMD_CONFIG: u8 = 18;
pub const CMD_RESET_MCU: u8 = 19;
pub const CMD_BOOTLOADER: u8 = 20;

// ── Helpers to build the first protocol byte ─────────────────────────────────

/// Build a command byte: `(COMMAND << 6) | sub_cmd`.
#[inline]
pub const fn command_byte(sub_cmd: u8) -> u8 {
    (OP_COMMAND << 6) | sub_cmd
}

/// Build a cycled-write header byte with a byte count in the lower 6 bits.
#[inline]
pub const fn cycled_write_header(count: u8) -> u8 {
    (OP_CYCLED_WRITE << 6) | count
}

/// Build a plain-write header byte with a byte count in the lower 6 bits.
#[inline]
pub const fn write_header(count: u8) -> u8 {
    (OP_WRITE << 6) | count
}

/// Build a read header byte.
#[inline]
pub const fn read_header() -> u8 {
    OP_READ << 6
}

// ── Line encoding (baud rate is ignored by TinyUSB) ──────────────────────────

/// CDC line encoding bytes: 9 000 000 baud, 8N1.
pub const LINE_ENCODING: [u8; 7] = [0x40, 0x54, 0x89, 0x00, 0x00, 0x00, 0x08];

// ── Clock speeds ─────────────────────────────────────────────────────────────

/// CPU clock speed presets (cycles per second).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum ClockSpeed {
    /// 1 MHz fallback.
    Default = 1_000_000,
    /// PAL – 0.985 248 MHz.
    Pal = 985_248,
    /// NTSC – 1.022 727 MHz.
    Ntsc = 1_022_727,
    /// DREAN – 1.023 440 MHz.
    Drean = 1_023_440,
    /// NTSC variant 2 – 1.022 730 MHz.
    Ntsc2 = 1_022_730,
}

/// Refresh rates in cycles (microsecond-scale).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum RefreshRate {
    /// 50 Hz fallback (20 000 cycles).
    Default = 20_000,
    /// PAL ~50.125 Hz (19 950 cycles).
    Eu = 19_950,
    /// NTSC ~59.826 Hz (16 715 cycles).
    Us = 16_715,
}

/// Raster rates in cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum RasterRate {
    /// Fallback (20 000 cycles).
    Default = 20_000,
    /// PAL: 63 cycles × 312 lines = 19 656 cycles/frame @ 985 248 Hz ≈ 50.12 Hz.
    Eu = 19_656,
    /// NTSC: 65 cycles × 263 lines = 17 096 cycles/frame @ 1 022 727 Hz ≈ 59.83 Hz.
    Us = 17_096,
}

/// Ordered table that maps a clock-speed index to its associated rates.
/// Index 0 = Default, 1 = PAL, 2 = NTSC, 3 = DREAN, 4 = NTSC2.
pub const CLOCK_TABLE: [(ClockSpeed, RefreshRate, RasterRate); 5] = [
    (
        ClockSpeed::Default,
        RefreshRate::Default,
        RasterRate::Default,
    ),
    (ClockSpeed::Pal, RefreshRate::Eu, RasterRate::Eu),
    (ClockSpeed::Ntsc, RefreshRate::Us, RasterRate::Us),
    (ClockSpeed::Drean, RefreshRate::Us, RasterRate::Us),
    (ClockSpeed::Ntsc2, RefreshRate::Us, RasterRate::Us),
];

// ── Ring-buffer defaults ─────────────────────────────────────────────────────

pub const MIN_DIFF_SIZE: usize = 16;
pub const MIN_RING_SIZE: usize = 256;
pub const DEFAULT_DIFF_SIZE: usize = 64;
pub const DEFAULT_RING_SIZE: usize = 32768;

// ── SID address helpers ──────────────────────────────────────────────────────

/// Translate a 16-bit C64 address into the 8-bit register byte the device expects.
///
/// Maps `$D400–$D499` directly, and folds `$D500`/`$DE00`/`$DF00` ranges
/// into the second-SID region at `$D440`.
#[inline]
pub fn sid_address(addr: u16) -> u8 {
    const SID_LMASK: u16 = 0xFF;
    const SID2_MASK: u16 = 0x3F;
    const SID3_ADDR: u16 = 0xD440;

    match addr {
        0xD400..=0xD499 => (addr & SID_LMASK) as u8,
        0xD500..=0xD599 | 0xDE00..=0xDF99 => ((SID3_ADDR | (addr & SID2_MASK)) & SID_LMASK) as u8,
        _ => (addr & SID_LMASK) as u8,
    }
}
