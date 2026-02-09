// USBSID-Pico – .sid file player example
//
// Parses a PSID/RSID file (v1–v4), loads the 6502 binary into emulated
// C64 memory, runs the init routine, and calls the play routine at
// 50 Hz (PAL) or 60 Hz (NTSC), forwarding SID register writes to
// real hardware via the USBSID-Pico.
//
// Supports:
//   - Single SID (mono) tunes
//   - 2SID / stereo tunes (v3+ header, automatic detection)
//     Correctly handles SID2 at $D420, $D500, $DE00, etc.
//   - --stereo flag to mirror a mono tune to both SID chips
//
// Usage:
//   cargo run --example sid_player -- path/to/tune.sid [song_number] [--stereo]
//
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

use usbsid_pico::{sid_address, ClockSpeed, UsbSid};

/// Maximum USB packet size for the USBSID-Pico OUT endpoint.
const USB_PACKET_SIZE: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
//  Batched USB write
// ─────────────────────────────────────────────────────────────────────────────

/// Pack (reg, val) pairs into 64-byte USB bulk packets.
///
/// Protocol: `[header, reg, val, reg, val, ...]`
/// Header byte = data_length (opcode WRITE = 0, so `(0 << 6) | len`).
/// Max 31 pairs per 64-byte packet.
fn flush_writes(usbsid: &mut UsbSid, writes: &[(u8, u8)]) {
    if writes.is_empty() {
        return;
    }
    let max_pairs = (USB_PACKET_SIZE - 1) / 2; // 31

    for chunk in writes.chunks(max_pairs) {
        let data_len = chunk.len() * 2;
        let mut buf = vec![0u8; 1 + data_len];
        buf[0] = data_len as u8;
        for (i, &(reg, val)) in chunk.iter().enumerate() {
            buf[1 + i * 2] = reg;
            buf[2 + i * 2] = val;
        }
        let _ = usbsid.single_write(&buf);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  PSID / RSID header parser (v1–v4)
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
    /// C64 address of the second SID (0 = none / mono).
    /// Parsed from header offset $7A (v3+).
    /// Common values: $D420, $D500, $DE00.
    sid2_address: u16,
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
    let mut sid2_address: u16 = 0;

    if version >= 2 && data.len() >= 0x7C {
        let flags = read_be_u16(data, 0x76);
        is_pal = ((flags >> 2) & 0x03) != 2;

        // v3+: second SID address at offset $7A
        // Byte encodes the middle nybbles of $Dxx0:
        //   0x42 → $D420, 0x50 → $D500, 0xE0 → $DE00
        if version >= 3 && data.len() > 0x7A {
            let sid2_byte = data[0x7A];
            if sid2_byte >= 0x42 {
                sid2_address = 0xD000 | ((sid2_byte as u16) << 4);
            }
        }
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
        sid2_address,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
//  C64 memory bus – intercepts SID writes and translates via sid_address()
// ─────────────────────────────────────────────────────────────────────────────

/// 64 KB C64 memory with SID write interception.
///
/// Uses the driver's `sid_address()` to translate C64 addresses to
/// USBSID register bytes:
///
///   $D400–$D41F  → USBSID $00–$1F  (SID1)
///   $D420–$D43F  → USBSID $20–$3F  (SID2 when mapped at $D420)
///   $D500–$D51F  → USBSID $40–$5F  (SID2 when mapped at $D500)
///   $DE00–$DE1F  → USBSID $40–$5F  (SID2 when mapped at $DE00)
struct C64Memory {
    ram: [u8; 65536],
    /// Collected SID writes: (usbsid_register, value).
    pub sid_writes: Vec<(u8, u8)>,
    /// C64 base address of the second SID (0 = mono).
    sid2_base: u16,
    /// End of SID2 range (sid2_base + 0x1F).
    sid2_end: u16,
}

impl C64Memory {
    fn new(is_pal: bool, sid2_base: u16) -> Self {
        let mut ram = [0u8; 65536];
        ram[0x0001] = 0x37;
        ram[0x02A6] = if is_pal { 0x01 } else { 0x00 };
        let sid2_end = if sid2_base != 0 { sid2_base + 0x1F } else { 0 };
        Self {
            ram,
            sid_writes: Vec::with_capacity(256),
            sid2_base,
            sid2_end,
        }
    }

    fn load(&mut self, addr: u16, data: &[u8]) {
        let a = addr as usize;
        self.ram[a..a + data.len()].copy_from_slice(data);
    }

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
        self.ram[address as usize] = value;

        if self.sid2_base != 0 {
            // ── 2SID mode ────────────────────────────────────────────
            // Capture writes to SID1 ($D400–$D41F) and SID2 range only.
            // Skip SID1 mirror writes to avoid USBSID register conflicts.
            if address >= 0xD400 && address <= 0xD41F {
                // SID1 direct
                let reg = sid_address(address);
                self.sid_writes.push((reg, value));
            } else if address >= self.sid2_base && address <= self.sid2_end {
                // SID2 — sid_address() maps to correct USBSID offset
                let reg = sid_address(address);
                self.sid_writes.push((reg, value));
            }
        } else {
            // ── Mono mode ────────────────────────────────────────────
            if address >= 0xD400 && address <= 0xD7FF {
                let reg = (address as u8) & 0x1F;
                self.sid_writes.push((reg, value));
            }
        }
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
        eprintln!();
        eprintln!("  --stereo   Mirror mono SID to both channels (not needed for 2SID files)");
        process::exit(1);
    }

    let force_stereo = args.iter().any(|a| a == "--stereo");

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

    let is_2sid = header.sid2_address != 0;
    let use_stereo = is_2sid || force_stereo;

    // Compute USBSID register offset for SID2 volume register.
    // sid_address() maps:
    //   $D420+$18 = $D438 → 0x38  (SID2 at $D420 → regs $20–$3F)
    //   $D500+$18 = $D518 → 0x58  (SID2 at $D500 → regs $40–$5F)
    //   $DE00+$18 = $DE18 → 0x58  (SID2 at $DE00 → regs $40–$5F)
    let sid2_vol_reg = if is_2sid {
        sid_address(header.sid2_address + 0x18)
    } else {
        sid_address(0xD420 + 0x18) // default for --stereo mirror
    };
    let sid2_base_reg = sid2_vol_reg & 0xE0; // $20 or $40

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
    if is_2sid {
        println!(
            "│  SID2   : ${:04X} → USBSID regs ${:02X}–${:02X}",
            header.sid2_address,
            sid2_base_reg,
            sid2_base_reg + 0x1F
        );
    }
    println!(
        "│  Output : {}",
        match (is_2sid, force_stereo) {
            (true, _) => "STEREO (native 2SID)",
            (false, true) => "STEREO (mono mirrored)",
            (false, false) => "MONO",
        }
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
    let sid2_for_bus = if is_2sid { header.sid2_address } else { 0 };
    let mut mem = C64Memory::new(header.is_pal, sid2_for_bus);
    mem.load(load_addr, payload);

    let trampoline: u16 = 0x0300;
    let halt_pc = trampoline + 3;

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

    if use_stereo {
        usbsid.set_stereo(1);
        let _ = usbsid.write(0x18, 0x0F); // SID1 volume max
        let _ = usbsid.write(sid2_vol_reg, 0x0F); // SID2 volume max
    } else {
        usbsid.set_stereo(0);
        let _ = usbsid.write(0x18, 0x0F);
    }

    // ── INIT ─────────────────────────────────────────────────────────────
    mem.install_trampoline(trampoline, header.init_address);
    let mut cpu = CPU::new(mem, Nmos6502);
    cpu.registers.program_counter = trampoline;
    cpu.registers.stack_pointer = StackPointer(0xFD);
    cpu.registers.accumulator = song.saturating_sub(1) as u8;

    run_until(&mut cpu, halt_pc, 2_000_000);

    // Send init writes (few, so individual transfers are fine)
    for &(reg, val) in &cpu.memory.sid_writes {
        let _ = usbsid.write(reg, val);
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

    let mirror_mono = force_stereo && !is_2sid;

    println!("  Playing... (Ctrl+C to stop)\n");

    while running.load(Ordering::Relaxed) {
        let t = Instant::now();

        cpu.registers.program_counter = trampoline;
        cpu.registers.stack_pointer = StackPointer(0xFD);
        cpu.memory.clear_writes();

        run_until(&mut cpu, halt_pc, 200_000);

        if mirror_mono {
            // Mono mirroring: duplicate SID1 writes to SID2
            let mut all: Vec<(u8, u8)> = Vec::with_capacity(cpu.memory.sid_writes.len() * 2);
            for &(reg, val) in &cpu.memory.sid_writes {
                all.push((reg, val));
                if reg <= 0x18 {
                    all.push((reg + sid2_base_reg, val));
                }
            }
            flush_writes(&mut usbsid, &all);
        } else {
            // 2SID or plain mono: sid_address() already set correct register bytes
            flush_writes(&mut usbsid, &cpu.memory.sid_writes);
        }

        let secs = t0.elapsed().as_secs();
        print!(
            "\r  ▶ {:02}:{:02}  {} SID writes",
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
    if use_stereo {
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
