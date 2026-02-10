// Direct register write example
//
// Plays a simple arpeggio on SID voice 1 using direct register writes.
// No .sid file or 6502 emulation required – this talks to the SID chip
// directly, which is a great starting point for integration.
//
// Run with:
//   cargo run --example simple_tone
//
// Requires a USBSID-Pico device connected via USB.

use std::thread;
use std::time::Duration;

use usbsid_pico::{ClockSpeed, UsbSid};

/// SID register offsets (voice 1 starts at $D400, so offset 0x00).
#[allow(dead_code)]
mod sid {
    // Voice 1
    pub const V1_FREQ_LO: u8 = 0x00;
    pub const V1_FREQ_HI: u8 = 0x01;
    pub const V1_PW_LO: u8 = 0x02;
    pub const V1_PW_HI: u8 = 0x03;
    pub const V1_CTRL: u8 = 0x04;
    pub const V1_AD: u8 = 0x05;
    pub const V1_SR: u8 = 0x06;

    // Voice 2
    pub const V2_FREQ_LO: u8 = 0x07;
    pub const V2_FREQ_HI: u8 = 0x08;
    pub const V2_PW_LO: u8 = 0x09;
    pub const V2_PW_HI: u8 = 0x0A;
    pub const V2_CTRL: u8 = 0x0B;
    pub const V2_AD: u8 = 0x0C;
    pub const V2_SR: u8 = 0x0D;

    // Filter / volume
    pub const FILT_LO: u8 = 0x15;
    pub const FILT_HI: u8 = 0x16;
    pub const RES_FILT: u8 = 0x17;
    pub const MODE_VOL: u8 = 0x18;

    // Control register bits
    pub const GATE: u8 = 0x01;
    pub const TRIANGLE: u8 = 0x10;
    pub const SAWTOOTH: u8 = 0x20;
    pub const PULSE: u8 = 0x40;
    pub const NOISE: u8 = 0x80;
}

/// Note frequency table (PAL, selected notes).
/// These are the 16-bit values written to FREQ_HI:FREQ_LO.
const fn note_freq(hi: u8, lo: u8) -> (u8, u8) {
    (hi, lo)
}

fn main() {
    env_logger::init();

    // ── Connect to USBSID-Pico ──────────────────────────────────────────
    let mut us = UsbSid::new();

    // Non-threaded mode: we do direct writes with explicit timing.
    if let Err(e) = us.init(false, false) {
        eprintln!("Failed to init USBSID: {e}");
        eprintln!("Make sure the device is connected and you have USB permissions.");
        return;
    }

    println!("USBSID-Pico connected!");

    // Set PAL clock
    us.set_clock_rate(ClockSpeed::Pal as i64, true);

    // Reset everything
    us.reset();
    thread::sleep(Duration::from_millis(50));

    // ── Configure SID ────────────────────────────────────────────────────

    // Set master volume to max (15)
    let _ = us.write(sid::MODE_VOL, 0x0F);

    // Voice 1: pulse wave, pulse width 50%
    let _ = us.write(sid::V1_PW_LO, 0x00);
    let _ = us.write(sid::V1_PW_HI, 0x08); // 50% duty cycle
    let _ = us.write(sid::V1_AD, 0x09); // Attack=0, Decay=9
    let _ = us.write(sid::V1_SR, 0x00); // Sustain=0, Release=0

    // Voice 2: sawtooth, slightly detuned for thickness
    let _ = us.write(sid::V2_AD, 0x0A);
    let _ = us.write(sid::V2_SR, 0x00);

    println!("Playing arpeggio... Press Ctrl+C to stop.");

    // ── Note table (C major arpeggio across octaves) ─────────────────────
    // Format: (freq_hi, freq_lo) — values for PAL SID
    let notes: &[(u8, u8)] = &[
        note_freq(0x04, 0x3E), // C3
        note_freq(0x05, 0x11), // E3
        note_freq(0x06, 0x0A), // G3
        note_freq(0x08, 0x7C), // C4
        note_freq(0x0A, 0x22), // E4
        note_freq(0x0C, 0x14), // G4
        note_freq(0x10, 0xF8), // C5
        note_freq(0x0C, 0x14), // G4
        note_freq(0x0A, 0x22), // E4
        note_freq(0x08, 0x7C), // C4
        note_freq(0x06, 0x0A), // G3
        note_freq(0x05, 0x11), // E3
    ];

    // ── Play loop ────────────────────────────────────────────────────────
    for round in 0..4 {
        for &(hi, lo) in notes.iter() {
            // Voice 1: pulse
            let _ = us.write(sid::V1_FREQ_LO, lo);
            let _ = us.write(sid::V1_FREQ_HI, hi);
            let _ = us.write(sid::V1_CTRL, sid::PULSE | sid::GATE);

            // Voice 2: sawtooth, detuned +2
            let _ = us.write(sid::V2_FREQ_LO, lo.wrapping_add(2));
            let _ = us.write(sid::V2_FREQ_HI, hi);
            let _ = us.write(sid::V2_CTRL, sid::SAWTOOTH | sid::GATE);

            thread::sleep(Duration::from_millis(80));

            // Release gate
            let _ = us.write(sid::V1_CTRL, sid::PULSE);
            let _ = us.write(sid::V2_CTRL, sid::SAWTOOTH);

            thread::sleep(Duration::from_millis(20));
        }

        // Last round: play a held chord
        if round == 3 {
            let _ = us.write(sid::V1_AD, 0x2A);
            let _ = us.write(sid::V1_SR, 0xA8);
            let _ = us.write(sid::V1_FREQ_LO, 0x7C);
            let _ = us.write(sid::V1_FREQ_HI, 0x08); // C4
            let _ = us.write(sid::V1_CTRL, sid::PULSE | sid::GATE);

            let _ = us.write(sid::V2_AD, 0x2A);
            let _ = us.write(sid::V2_SR, 0xA8);
            let _ = us.write(sid::V2_FREQ_LO, 0x22);
            let _ = us.write(sid::V2_FREQ_HI, 0x0A); // E4
            let _ = us.write(sid::V2_CTRL, sid::SAWTOOTH | sid::GATE);

            thread::sleep(Duration::from_secs(2));

            let _ = us.write(sid::V1_CTRL, sid::PULSE);
            let _ = us.write(sid::V2_CTRL, sid::SAWTOOTH);
            thread::sleep(Duration::from_millis(500));
        }
    }

    // ── Cleanup ──────────────────────────────────────────────────────────
    us.mute();
    us.reset();
    us.close();
    println!("Done.");
}
