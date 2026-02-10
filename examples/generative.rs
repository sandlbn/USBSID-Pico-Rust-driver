// USBSID-Pico – Generative SID synthesizer
//
// Algorithmic music generator using Euclidean rhythms, selectable scales,
// and direct SID register control. Each SID voice runs an independent
// Euclidean pattern with notes picked from the active scale.
//
// Usage:
//   cargo run --example generative -- [options]
//
// Options:
//   --bpm 120          Set tempo (default: 120)
//   --scale minor      Scale: major, minor, dorian, pentatonic, chromatic
//   --steps 16         Euclidean sequence length (default: 16)
//   --pulses 5         Euclidean hits per sequence (default: 5)
//   --root C3          Root note (default: C3)
//   --stereo           Mirror to SID2

use std::env;
use std::io::Write;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use usbsid_pico::{ClockSpeed, UsbSid};

//  SID register map

#[allow(dead_code)]
mod sid {
    pub const FREQ_LO: u8 = 0x00;
    pub const FREQ_HI: u8 = 0x01;
    pub const PW_LO: u8 = 0x02;
    pub const PW_HI: u8 = 0x03;
    pub const CTRL: u8 = 0x04;
    pub const AD: u8 = 0x05;
    pub const SR: u8 = 0x06;
    pub const VOICE_SIZE: u8 = 7;

    pub const FILT_LO: u8 = 0x15;
    pub const FILT_HI: u8 = 0x16;
    pub const RES_FILT: u8 = 0x17;
    pub const MODE_VOL: u8 = 0x18;

    pub const GATE: u8 = 0x01;
    pub const SYNC: u8 = 0x02;
    pub const RING_MOD: u8 = 0x04;
    pub const TRIANGLE: u8 = 0x10;
    pub const SAWTOOTH: u8 = 0x20;
    pub const PULSE: u8 = 0x40;
    pub const NOISE: u8 = 0x80;

    /// Voice N register base (0, 1, or 2).
    pub const fn voice(n: u8) -> u8 {
        n * VOICE_SIZE
    }
}

//  Musical scales (semitone intervals from root)

#[derive(Clone, Copy, Debug)]
enum Scale {
    Major,
    Minor,
    Dorian,
    Pentatonic,
    PentatonicMinor,
    Chromatic,
    HarmonicMinor,
    Blues,
    Mixolydian,
}

impl Scale {
    fn intervals(&self) -> &'static [u8] {
        match self {
            Scale::Major => &[0, 2, 4, 5, 7, 9, 11],
            Scale::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Scale::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Scale::Pentatonic => &[0, 2, 4, 7, 9],
            Scale::PentatonicMinor => &[0, 3, 5, 7, 10],
            Scale::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            Scale::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Scale::Blues => &[0, 3, 5, 6, 7, 10],
            Scale::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "major" => Some(Scale::Major),
            "minor" => Some(Scale::Minor),
            "dorian" => Some(Scale::Dorian),
            "pentatonic" => Some(Scale::Pentatonic),
            "pentatonic_minor" => Some(Scale::PentatonicMinor),
            "chromatic" => Some(Scale::Chromatic),
            "harmonic_minor" => Some(Scale::HarmonicMinor),
            "blues" => Some(Scale::Blues),
            "mixolydian" => Some(Scale::Mixolydian),
            _ => None,
        }
    }
}

//  PAL SID frequency table (MIDI note 0–95 → SID freq register value)

/// PAL SID frequency = (f * 16777216) / 985248
/// where f is the desired frequency in Hz.
fn midi_to_sid_freq(midi_note: u8) -> u16 {
    let freq_hz = 440.0 * f64::powf(2.0, (midi_note as f64 - 69.0) / 12.0);
    (freq_hz * 16777216.0 / 985248.0) as u16
}

/// Convert a note name like "C3", "A#4", "Eb2" to MIDI number.
fn note_name_to_midi(s: &str) -> Option<u8> {
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.len() < 2 {
        return None;
    }

    let (note_base, rest) = match bytes[0] {
        b'C' | b'c' => (0, &s[1..]),
        b'D' | b'd' => (2, &s[1..]),
        b'E' | b'e' => (4, &s[1..]),
        b'F' | b'f' => (5, &s[1..]),
        b'G' | b'g' => (7, &s[1..]),
        b'A' | b'a' => (9, &s[1..]),
        b'B' | b'b' => (11, &s[1..]),
        _ => return None,
    };

    let (accidental, octave_str) = if let Some(stripped) = rest.strip_prefix('#') {
        (1i8, stripped)
    } else if let Some(stripped) = rest.strip_prefix('b') {
        (-1i8, stripped)
    } else {
        (0i8, rest)
    };

    let octave: i8 = octave_str.parse().ok()?;
    let midi = (octave + 1) as i16 * 12 + note_base as i16 + accidental as i16;
    if (0..=127).contains(&midi) {
        Some(midi as u8)
    } else {
        None
    }
}

//  Euclidean rhythm generator (Bjorklund's algorithm)

/// Generate a Euclidean rhythm: distribute `pulses` hits across `steps` slots.
///
/// Returns a Vec<bool> of length `steps` where `true` = hit.
///
/// Examples:
///   euclidean(8, 3)  → [X . . X . . X .]   (Cuban tresillo)
///   euclidean(8, 5)  → [X . X X . X X .]   (Cuban cinquillo)
///   euclidean(16, 5) → [X . . X . . X . . X . . X . . .]
fn euclidean(steps: usize, pulses: usize) -> Vec<bool> {
    if steps == 0 {
        return vec![];
    }
    if pulses >= steps {
        return vec![true; steps];
    }
    if pulses == 0 {
        return vec![false; steps];
    }

    // Bjorklund's algorithm
    let mut pattern: Vec<Vec<bool>> = Vec::new();
    let mut remainder: Vec<Vec<bool>> = Vec::new();

    for _ in 0..pulses {
        pattern.push(vec![true]);
    }
    for _ in 0..(steps - pulses) {
        remainder.push(vec![false]);
    }

    loop {
        let min_len = pattern.len().min(remainder.len());
        if remainder.len() <= 1 {
            break;
        }

        let mut new_pattern = Vec::new();
        for i in 0..min_len {
            let mut combined = pattern[i].clone();
            combined.extend_from_slice(&remainder[i]);
            new_pattern.push(combined);
        }

        let leftover_pattern: Vec<Vec<bool>> = pattern[min_len..].to_vec();
        let leftover_remainder: Vec<Vec<bool>> = remainder[min_len..].to_vec();

        pattern = new_pattern;
        remainder = if !leftover_pattern.is_empty() {
            leftover_pattern
        } else {
            leftover_remainder
        };
    }

    pattern.extend(remainder);
    pattern.into_iter().flatten().collect()
}

//  Scale-aware note picker

/// Build a note pool: N octaves of the given scale starting from root_midi.
fn build_note_pool(root_midi: u8, scale: Scale, octaves: u8) -> Vec<u8> {
    let intervals = scale.intervals();
    let mut notes = Vec::new();
    for oct in 0..octaves {
        for &interval in intervals {
            let note = root_midi + oct * 12 + interval;
            if note <= 127 {
                notes.push(note);
            }
        }
    }
    notes
}

//  Simple PRNG (xorshift32) – deterministic, no dependency

struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        Self(if seed == 0 { 1 } else { seed })
    }

    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    /// Random usize in [0, n)
    fn range(&mut self, n: usize) -> usize {
        (self.next() as usize) % n
    }

    /// Random f32 in [0.0, 1.0)
    fn float(&mut self) -> f32 {
        (self.next() & 0x00FF_FFFF) as f32 / 16777216.0
    }
}

//  Voice configuration

#[derive(Clone)]
struct VoiceConfig {
    /// Euclidean pattern for this voice.
    pattern: Vec<bool>,
    /// Current step index in the pattern.
    step: usize,
    /// Waveform control byte (PULSE, SAWTOOTH, TRIANGLE, NOISE).
    waveform: u8,
    /// ADSR bytes.
    attack_decay: u8,
    sustain_release: u8,
    /// Pulse width (0x000–0xFFF).
    pulse_width: u16,
    /// Octave offset from root for this voice.
    octave_offset: i8,
    /// Probability (0.0–1.0) that a hit actually triggers.
    probability: f32,
    /// Note pool subset indices to pick from (empty = all).
    note_range: (usize, usize),
}

//  Main

fn main() {
    env_logger::init();

    let args: Vec<String> = env::args().collect();

    // Parse CLI options
    let bpm = parse_opt(&args, "--bpm").unwrap_or(120u32);
    let scale = args
        .windows(2)
        .find(|w| w[0] == "--scale")
        .and_then(|w| Scale::from_str(&w[1]))
        .unwrap_or(Scale::PentatonicMinor);
    let steps = parse_opt(&args, "--steps").unwrap_or(16usize);
    let pulses_v1 = parse_opt(&args, "--pulses").unwrap_or(5usize);
    let root_str = args
        .windows(2)
        .find(|w| w[0] == "--root")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "C3".into());
    let root_midi = note_name_to_midi(&root_str).unwrap_or(48); // C3
    let force_stereo = args.iter().any(|a| a == "--stereo");
    let seed = parse_opt(&args, "--seed").unwrap_or(42u32);

    // Build note pool (3 octaves)
    let note_pool = build_note_pool(root_midi, scale, 3);

    let mut rng = Rng::new(seed);

    // ── Configure 3 voices with different Euclidean patterns ─────────
    //
    // Voice 0: bass – sparse hits, low octave, triangle
    // Voice 1: lead – medium density, mid octave, pulse
    // Voice 2: perc – dense/noisy, high octave, noise

    let mut voices = [
        VoiceConfig {
            pattern: euclidean(steps, pulses_v1.min(steps)),
            step: 0,
            waveform: sid::TRIANGLE,
            attack_decay: 0x09,    // A=0, D=9
            sustain_release: 0xA0, // S=10, R=0
            pulse_width: 0x0800,
            octave_offset: -1,
            probability: 0.9,
            note_range: (0, note_pool.len().min(7)),
        },
        VoiceConfig {
            pattern: euclidean(steps, (pulses_v1 + 2).min(steps)),
            step: 0,
            waveform: sid::PULSE,
            attack_decay: 0x2A,    // A=2, D=10
            sustain_release: 0x80, // S=8, R=0
            pulse_width: 0x0600,
            octave_offset: 0,
            probability: 0.8,
            note_range: (3, note_pool.len().min(12)),
        },
        VoiceConfig {
            pattern: euclidean(steps, (pulses_v1 + 4).min(steps)),
            step: 0,
            waveform: sid::NOISE,
            attack_decay: 0x00,    // A=0, D=0
            sustain_release: 0x00, // S=0, R=0
            pulse_width: 0x0800,
            octave_offset: 1,
            probability: 0.6,
            note_range: (5, note_pool.len()),
        },
    ];

    // ── Print config ─────────────────────────────────────────────────
    println!("┌────────────────────────────────────────────────┐");
    println!("│  USBSID-Pico Generative Synth                 │");
    println!("├────────────────────────────────────────────────┤");
    println!("│  BPM   : {bpm}");
    println!("│  Scale : {scale:?}");
    println!("│  Root  : {root_str} (MIDI {root_midi})");
    println!("│  Steps : {steps}");
    println!("│  Notes : {} in pool across 3 octaves", note_pool.len());

    for (i, v) in voices.iter().enumerate() {
        let pat: String = v
            .pattern
            .iter()
            .map(|&b| if b { 'X' } else { '.' })
            .collect();
        let wave = match v.waveform {
            sid::TRIANGLE => "TRI",
            sid::SAWTOOTH => "SAW",
            sid::PULSE => "PUL",
            sid::NOISE => "NOI",
            _ => "???",
        };
        println!(
            "│  V{i}    : [{pat}] {wave} prob={:.0}%",
            v.probability * 100.0
        );
    }
    println!("└────────────────────────────────────────────────┘");

    // ── Connect ──────────────────────────────────────────────────────
    let mut us = UsbSid::new();
    if let Err(e) = us.init(false, false) {
        eprintln!("USBSID init failed: {e}");
        process::exit(1);
    }

    us.set_clock_rate(ClockSpeed::Pal as i64, true);
    us.reset();
    thread::sleep(Duration::from_millis(50));

    if force_stereo {
        us.set_stereo(1);
    }

    // Max volume
    let _ = us.write(sid::MODE_VOL, 0x0F);
    if force_stereo {
        let _ = us.write(0x20 + sid::MODE_VOL, 0x0F);
    }

    // Set up voices
    for (i, v) in voices.iter().enumerate() {
        let base = sid::voice(i as u8);
        let _ = us.write(base + sid::PW_LO, (v.pulse_width & 0xFF) as u8);
        let _ = us.write(base + sid::PW_HI, ((v.pulse_width >> 8) & 0x0F) as u8);
        let _ = us.write(base + sid::AD, v.attack_decay);
        let _ = us.write(base + sid::SR, v.sustain_release);
    }

    // ── Ctrl+C ───────────────────────────────────────────────────────
    let running = Arc::new(AtomicBool::new(true));
    #[cfg(unix)]
    {
        let r = running.clone();
        unsafe {
            RUNNING_FLAG = Some(r);
            libc::signal(libc::SIGINT, signal_handler as libc::sighandler_t);
        }
    }

    // ── Sequencer loop ───────────────────────────────────────────────
    // Each step = one 16th note at the given BPM
    let step_duration = Duration::from_secs_f64(60.0 / bpm as f64 / 4.0);
    let gate_on_frac = 0.7; // 70% gate on, 30% gate off

    println!("\n  Playing... (Ctrl+C to stop)\n");

    let mut global_step: u64 = 0;

    while running.load(Ordering::Relaxed) {
        let step_start = Instant::now();

        for (i, voice) in voices.iter_mut().enumerate() {
            let base = sid::voice(i as u8);
            let is_hit = voice.pattern[voice.step % voice.pattern.len()];

            if is_hit && rng.float() < voice.probability {
                // Pick a note from the pool
                let (lo, hi) = voice.note_range;
                let idx = lo + rng.range(hi.saturating_sub(lo).max(1));
                let midi_note = note_pool[idx.min(note_pool.len() - 1)] as i16
                    + (voice.octave_offset as i16 * 12);
                let midi_note = midi_note.clamp(0, 127) as u8;

                let freq = midi_to_sid_freq(midi_note);
                let _ = us.write(base + sid::FREQ_LO, (freq & 0xFF) as u8);
                let _ = us.write(base + sid::FREQ_HI, (freq >> 8) as u8);
                let _ = us.write(base + sid::CTRL, voice.waveform | sid::GATE);

                // Mirror to SID2 if stereo
                if force_stereo {
                    let s2 = 0x20 + base;
                    let _ = us.write(s2 + sid::FREQ_LO, (freq & 0xFF) as u8);
                    let _ = us.write(s2 + sid::FREQ_HI, (freq >> 8) as u8);
                    let _ = us.write(s2 + sid::CTRL, voice.waveform | sid::GATE);
                }
            }

            voice.step = (voice.step + 1) % voice.pattern.len();
        }

        // Status
        let bar = global_step / (steps as u64);
        let beat = global_step % (steps as u64);
        print!("\r  Bar {:3} Step {:2}/{}", bar + 1, beat + 1, steps);
        let _ = std::io::stdout().flush();

        // Gate ON duration
        let gate_on_dur = step_duration.mul_f64(gate_on_frac);
        let elapsed = step_start.elapsed();
        if elapsed < gate_on_dur {
            thread::sleep(gate_on_dur - elapsed);
        }

        // Gate OFF
        for (i, voice) in voices.iter().enumerate() {
            let base = sid::voice(i as u8);
            let _ = us.write(base + sid::CTRL, voice.waveform); // gate off
            if force_stereo {
                let _ = us.write(0x20 + base + sid::CTRL, voice.waveform);
            }
        }

        let elapsed = step_start.elapsed();
        if elapsed < step_duration {
            thread::sleep(step_duration - elapsed);
        }

        global_step += 1;

        // ── Evolve patterns every 4 bars ─────────────────────────────
        if global_step % (steps as u64 * 4) == 0 {
            // Mutate: randomly shift pulse count ±1 on a random voice
            let vi = rng.range(voices.len());
            let current_pulses = voices[vi].pattern.iter().filter(|&&b| b).count();
            let new_pulses = if rng.float() > 0.5 {
                (current_pulses + 1).min(steps)
            } else {
                current_pulses.saturating_sub(1).max(1)
            };
            voices[vi].pattern = euclidean(steps, new_pulses);
            voices[vi].step = 0;

            // Occasionally shift pulse width for timbral variation
            if rng.float() > 0.7 {
                let pw = 0x0200 + (rng.next() as u16 % 0x0C00);
                voices[vi].pulse_width = pw;
                let base = sid::voice(vi as u8);
                let _ = us.write(base + sid::PW_LO, (pw & 0xFF) as u8);
                let _ = us.write(base + sid::PW_HI, ((pw >> 8) & 0x0F) as u8);
            }
        }
    }

    // ── Cleanup ──────────────────────────────────────────────────────
    println!("\n\n  Stopping...");
    us.mute();
    if force_stereo {
        us.set_stereo(0);
    }
    us.reset();
    us.close();
    println!("  Done.");
}

fn parse_opt<T: std::str::FromStr>(args: &[String], flag: &str) -> Option<T> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .and_then(|w| w[1].parse().ok())
}

#[cfg(unix)]
static mut RUNNING_FLAG: Option<Arc<AtomicBool>> = None;

#[cfg(unix)]
extern "C" fn signal_handler(_: i32) {
    unsafe {
        if let Some(ref f) = RUNNING_FLAG {
            f.store(false, Ordering::Relaxed);
        }
    }
}
