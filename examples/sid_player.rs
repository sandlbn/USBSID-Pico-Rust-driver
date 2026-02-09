// USBSID-Pico – .sid file player example
//
// Parses a PSID/RSID file, loads the 6502 binary into emulated C64 memory,
// runs the init routine, and calls the play routine at 50 Hz (PAL) or
// 60 Hz (NTSC), forwarding every SID register write to real hardware
// via the USBSID-Pico.
//
// Usage:
//   cargo run --example sid_player -- path/to/tune.sid [song_number] [--stereo]
//
// Flags:
//   --stereo   Mirror SID writes to second SID chip (both speakers)
//
// Requires a connected USBSID-Pico device.
// Download .sid files from the High Voltage SID Collection: https://hvsc.c64.org

use std::env;
use std::fs;
use std::io::Write;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use mos6502::cpu::CPU;
use mos6502::instruction::Nmos6502;
use mos6502::memory::Bus;
use mos6502::registers::StackPointer;

use usbsid_pico::{ClockSpeed, UsbSid};

// ─────────────────────────────────────────────────────────────────────────────
//  PSID / RSID header parser
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct SidHeader {
    magic: String,
    version: u16,
    data_offset: u16,
    load_address: u16,
    init_address: u16,
    play_address: u16,
    songs: u16,
    start_song: u16,
    _speed: u32,
    name: String,
    author: String,
    released: String,
    is_pal: bool,
}

fn read_be_u16(d: &[u8], o: usize) -> u16 {
    ((d[o] as u16) << 8) | d[o + 1] as u16
}
fn read_be_u32(d: &[u8], o: usize) -> u32 {
    ((d[o] as u32) << 24) | ((d[o + 1] as u32) << 16) | ((d[o + 2] as u32) << 8) | d[o + 3] as u32
}
fn read_string(d: &[u8], o: usize, len: usize) -> String {
    let s = &d[o..o + len];
    let end = s.iter().position(|&b| b == 0).unwrap_or(len);
    String::from_utf8_lossy(&s[..end]).to_string()
}

fn parse_sid_header(data: &[u8]) -> Result<SidHeader, String> {
    if data.len() < 0x76 {
        return Err("File too small for a SID header".into());
    }
    let magic = String::from_utf8_lossy(&data[0..4]).to_string();
    if magic != "PSID" && magic != "RSID" {
        return Err(format!("Not a SID file (magic={magic:?})"));
    }

    let version = read_be_u16(data, 0x04);
    let mut is_pal = true;
    if version >= 2 && data.len() >= 0x7C {
        let flags = read_be_u16(data, 0x76);
        is_pal = ((flags >> 2) & 0x03) != 2;
    }

    Ok(SidHeader {
        magic,
        version,
        data_offset: read_be_u16(data, 0x06),
        load_address: read_be_u16(data, 0x08),
        init_address: read_be_u16(data, 0x0A),
        play_address: read_be_u16(data, 0x0C),
        songs: read_be_u16(data, 0x0E),
        start_song: read_be_u16(data, 0x10),
        _speed: read_be_u32(data, 0x12),
        name: read_string(data, 0x16, 32),
        author: read_string(data, 0x36, 32),
        released: read_string(data, 0x56, 32),
        is_pal,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
//  C64 memory bus – intercepts SID writes
// ─────────────────────────────────────────────────────────────────────────────

/// 64 KB C64 memory. Writes to $D400–$D7FF are captured so we can
/// replay them on real hardware after each play-routine frame.
struct C64Memory {
    ram: [u8; 65536],
    pub sid_writes: Vec<(u8, u8)>,
}

impl C64Memory {
    fn new(is_pal: bool) -> Self {
        let mut ram = [0u8; 65536];
        ram[0x0001] = 0x37;
        ram[0x02A6] = if is_pal { 0x01 } else { 0x00 };
        Self {
            ram,
            sid_writes: Vec::with_capacity(256),
        }
    }

    fn load(&mut self, addr: u16, data: &[u8]) {
        let a = addr as usize;
        self.ram[a..a + data.len()].copy_from_slice(data);
    }

    /// JSR target; JMP (self)  — a tiny call-and-halt stub.
    fn install_trampoline(&mut self, at: u16, target: u16) {
        let a = at as usize;
        self.ram[a] = 0x20; // JSR
        self.ram[a + 1] = (target & 0xFF) as u8;
        self.ram[a + 2] = (target >> 8) as u8;
        self.ram[a + 3] = 0x4C; // JMP (halt loop)
        self.ram[a + 4] = ((at + 3) & 0xFF) as u8;
        self.ram[a + 5] = ((at + 3) >> 8) as u8;
    }

    fn clear_writes(&mut self) {
        self.sid_writes.clear();
    }
}

impl Bus for C64Memory {
    fn get_byte(&mut self, address: u16) -> u8 {
        self.ram[address as usize]
    }
    fn set_byte(&mut self, address: u16, value: u8) {
        let a = address as usize;
        if (0xD400..=0xD7FF).contains(&a) {
            self.sid_writes.push(((a & 0xFF) as u8, value));
        }
        self.ram[a] = value;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Helper: run 6502 until PC hits `halt` or `max_steps` exceeded
// ─────────────────────────────────────────────────────────────────────────────

fn run_until(cpu: &mut CPU<C64Memory, Nmos6502>, halt: u16, max_steps: u32) {
    for _ in 0..max_steps {
        if cpu.registers.program_counter == halt {
            return;
        }
        cpu.single_step();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Main
// ─────────────────────────────────────────────────────────────────────────────

fn main() {
    env_logger::init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <file.sid> [song_number] [--stereo]", args[0]);
        process::exit(1);
    }

    let stereo = args.iter().any(|a| a == "--stereo");
    // Second SID registers start at offset 0x20 on the USBSID-Pico
    const SID2_OFFSET: u8 = 0x20;

    let file_data = fs::read(&args[1]).unwrap_or_else(|e| {
        eprintln!("Cannot read {}: {e}", args[1]);
        process::exit(1);
    });
    let header = parse_sid_header(&file_data).unwrap_or_else(|e| {
        eprintln!("SID error: {e}");
        process::exit(1);
    });

    let song = args
        .iter()
        .skip(2)
        .find(|a| !a.starts_with("--"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(header.start_song);

    println!("┌────────────────────────────────────────────────┐");
    println!("│  USBSID-Pico .SID Player                      │");
    println!("├────────────────────────────────────────────────┤");
    println!(
        "│  {} v{}  │  {} ",
        header.magic,
        header.version,
        if header.is_pal {
            "PAL 50 Hz"
        } else {
            "NTSC 60 Hz"
        }
    );
    println!("│  Title  : {}", header.name);
    println!("│  Author : {}", header.author);
    println!("│  Release: {}", header.released);
    println!(
        "│  Songs  : {} (#{})  Init ${:04X}  Play ${:04X}",
        header.songs, song, header.init_address, header.play_address
    );
    println!(
        "│  Output : {}",
        if stereo { "STEREO (dual SID)" } else { "MONO" }
    );
    println!("└────────────────────────────────────────────────┘");

    // ── Load payload ─────────────────────────────────────────────────────
    let ds = header.data_offset as usize;
    let (load_addr, payload_start) = if header.load_address == 0 {
        let lo = file_data[ds] as u16;
        let hi = file_data[ds + 1] as u16;
        ((hi << 8) | lo, ds + 2)
    } else {
        (header.load_address, ds)
    };
    let payload = &file_data[payload_start..];
    println!("  {} bytes → ${:04X}", payload.len(), load_addr);

    // ── C64 memory + CPU ─────────────────────────────────────────────────
    let mut mem = C64Memory::new(header.is_pal);
    mem.load(load_addr, payload);

    let trampoline: u16 = 0x0300;
    let halt_pc = trampoline + 3;

    // Point IRQ/NMI vectors at the halt loop
    mem.ram[0xFFFA] = (halt_pc & 0xFF) as u8;
    mem.ram[0xFFFB] = (halt_pc >> 8) as u8;
    mem.ram[0xFFFE] = (halt_pc & 0xFF) as u8;
    mem.ram[0xFFFF] = (halt_pc >> 8) as u8;

    // ── Connect hardware ─────────────────────────────────────────────────
    let mut usbsid = UsbSid::new();
    if let Err(e) = usbsid.init(false, false) {
        eprintln!("USBSID init failed: {e}");
        process::exit(1);
    }
    usbsid.set_clock_rate(
        if header.is_pal {
            ClockSpeed::Pal as i64
        } else {
            ClockSpeed::Ntsc as i64
        },
        true,
    );
    usbsid.reset();
    thread::sleep(Duration::from_millis(50));
    let _ = usbsid.write(0x18, 0x0F); // SID1 master volume max

    if stereo {
        usbsid.set_stereo(1);
        let _ = usbsid.write(SID2_OFFSET + 0x18, 0x0F); // SID2 master volume max
    } else {
        usbsid.set_stereo(0); // mono: route SID1 to both L+R channels
    }

    // ── INIT ─────────────────────────────────────────────────────────────
    mem.install_trampoline(trampoline, header.init_address);
    let mut cpu = CPU::new(mem, Nmos6502);
    cpu.registers.program_counter = trampoline;
    cpu.registers.stack_pointer = StackPointer(0xFD);
    cpu.registers.accumulator = song.saturating_sub(1) as u8;

    run_until(&mut cpu, halt_pc, 2_000_000);

    for &(reg, val) in &cpu.memory.sid_writes {
        let _ = usbsid.write(reg, val);
        if stereo && reg <= 0x18 {
            let _ = usbsid.write(reg + SID2_OFFSET, val);
        }
    }
    cpu.memory.clear_writes();
    println!("  Init done.");

    // ── Check play address ───────────────────────────────────────────────
    if header.play_address == 0 {
        eprintln!("  play_address=0 (IRQ-driven) not supported in this example.");
        usbsid.close();
        process::exit(1);
    }
    cpu.memory
        .install_trampoline(trampoline, header.play_address);

    // ── Ctrl+C ───────────────────────────────────────────────────────────
    let running = Arc::new(AtomicBool::new(true));
    #[cfg(unix)]
    {
        let r = running.clone();
        unsafe {
            RUNNING_FLAG = Some(r);
            libc::signal(libc::SIGINT, signal_handler as libc::sighandler_t);
        }
    }

    // ── Play loop ────────────────────────────────────────────────────────
    let frame_us = if header.is_pal { 20_000u64 } else { 16_667u64 };
    let frame_dur = Duration::from_micros(frame_us);
    let t0 = Instant::now();

    println!("  Playing... (Ctrl+C to stop)\n");

    while running.load(Ordering::Relaxed) {
        let t = Instant::now();

        cpu.registers.program_counter = trampoline;
        cpu.registers.stack_pointer = StackPointer(0xFD);
        cpu.memory.clear_writes();

        run_until(&mut cpu, halt_pc, 200_000);

        // Forward SID writes to real hardware
        for &(reg, val) in &cpu.memory.sid_writes {
            let _ = usbsid.write(reg, val);
            if stereo && reg <= 0x18 {
                let _ = usbsid.write(reg + SID2_OFFSET, val);
            }
        }

        let secs = t0.elapsed().as_secs();
        print!(
            "\r  ▶ {:02}:{:02}  {} writes/frame",
            secs / 60,
            secs % 60,
            cpu.memory.sid_writes.len()
        );
        let _ = std::io::stdout().flush();

        let elapsed = t.elapsed();
        if elapsed < frame_dur {
            thread::sleep(frame_dur - elapsed);
        }
    }

    println!("\n\n  Stopping...");
    usbsid.mute();
    if stereo {
        usbsid.set_stereo(0);
    }
    usbsid.reset();
    usbsid.close();
    println!("  Done.");
}

// ── SIGINT handler (Unix) ────────────────────────────────────────────────────

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
