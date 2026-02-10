// USBSID-Pico – .sid file player example
//
// Parses a PSID/RSID file (v1–v4), loads the 6502 binary into emulated
// C64 memory, runs the init routine, and calls the play routine at
// 50 Hz (PAL) or 60 Hz (NTSC), forwarding SID register writes to
// real hardware via the USBSID-Pico.
//
// Supports:
//   - 1 SID (mono) tunes
//   - 2SID stereo tunes (v3+ header, auto-detected)
//   - 3SID tunes (v4 header, auto-detected)
//   - 4SID via --sid4 flag (no standard header field)
//   - --stereo flag to mirror a mono tune to both SID chips
//
// USBSID-Pico register mapping (n * 0x20):
//   SID1 → regs $00–$1F   SID2 → regs $20–$3F
//   SID3 → regs $40–$5F   SID4 → regs $60–$7F
//
// Usage:
//   cargo run --example sid_player -- path/to/tune.sid [song_number] [flags]
//
// Flags:
//   --stereo       Mirror mono SID1 to SID2 (not needed for 2SID+ files)
//   --sid4 $Dxx0   Set 4th SID address (e.g. --sid4 $D440 or --sid4 0xD440)
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

use usbsid_pico::{ClockSpeed, UsbSid};

/// Maximum USB packet size for the USBSID-Pico OUT endpoint.
const USB_PACKET_SIZE: usize = 64;

/// Each SID occupies 0x20 registers on the USBSID-Pico.
/// SID N is at offset N * SID_REG_SIZE.
const SID_REG_SIZE: u8 = 0x20;

/// Volume register offset within each SID (register $18).
const SID_VOL_REG: u8 = 0x18;

// ─────────────────────────────────────────────────────────────────────────────
//  SID address → USBSID register mapping
// ─────────────────────────────────────────────────────────────────────────────

/// Maps C64 SID addresses to USBSID register numbers.
///
/// Each SID chip has a C64 address range ($D400, $D420, $D500, $DE00, etc.)
/// and a USBSID slot (0–3). The USBSID register = slot * 0x20 + (addr & 0x1F).
struct SidMapper {
    /// (c64_base, c64_end) for each SID slot.
    /// Slot 0 is always SID1 @ $D400.
    ranges: Vec<(u16, u16)>,
}

#[allow(dead_code)]
impl SidMapper {
    /// Create mapper from a list of C64 base addresses.
    /// First entry is always SID1 ($D400). Additional entries are SID2–SID4.
    fn new(bases: &[u16]) -> Self {
        let ranges = bases.iter().map(|&base| (base, base + 0x1F)).collect();
        Self { ranges }
    }

    /// Translate a C64 address to a USBSID register byte.
    /// Returns None if the address doesn't belong to any known SID.
    fn map(&self, addr: u16) -> Option<u8> {
        for (slot, &(base, end)) in self.ranges.iter().enumerate() {
            if addr >= base && addr <= end {
                let reg_offset = (addr - base) as u8;
                return Some((slot as u8) * SID_REG_SIZE + reg_offset);
            }
        }
        None
    }

    /// Number of SID chips configured.
    fn num_sids(&self) -> usize {
        self.ranges.len()
    }

    /// USBSID register for the volume register of SID N (0-based).
    fn vol_reg(&self, sid: usize) -> u8 {
        (sid as u8) * SID_REG_SIZE + SID_VOL_REG
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Batched USB write
// ─────────────────────────────────────────────────────────────────────────────

/// Pack (reg, val) pairs into 64-byte USB bulk packets.
fn flush_writes(usbsid: &mut UsbSid, writes: &[(u8, u8)]) {
    if writes.is_empty() {
        return;
    }
    let max_pairs = (USB_PACKET_SIZE - 1) / 2; // 31

    for chunk in writes.chunks(max_pairs) {
        let data_len = chunk.len() * 2;
        let mut buf = vec![0u8; USB_PACKET_SIZE]; // always full 64 bytes
        buf[0] = data_len as u8; // header: (OP_WRITE << 6) | len
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
    /// C64 addresses of extra SIDs (0 = unused). Index 0 = SID2, 1 = SID3.
    extra_sid_addrs: [u16; 2],
}

#[allow(dead_code)]
impl SidHeader {
    fn num_sids(&self) -> usize {
        1 + self.extra_sid_addrs.iter().filter(|&&a| a != 0).count()
    }
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

/// Decode a SID address byte (from header offset $7A or $7B).
/// Returns the C64 address or 0 if invalid.
fn decode_sid_addr_byte(b: u8) -> u16 {
    // Valid: 0x42–0x7F and 0xE0–0xFE (even only)
    if b >= 0x42 && (b <= 0x7F || b >= 0xE0) && (b & 1) == 0 {
        0xD000 | ((b as u16) << 4)
    } else {
        0
    }
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
    let mut extra_sid_addrs = [0u16; 2];

    if version >= 2 && data.len() >= 0x7C {
        let flags = read_be_u16(data, 0x76);
        is_pal = ((flags >> 2) & 0x03) != 2;

        // v3+: second SID at $7A
        if version >= 3 && data.len() > 0x7A {
            extra_sid_addrs[0] = decode_sid_addr_byte(data[0x7A]);
        }
        // v4: third SID at $7B
        if version >= 4 && data.len() > 0x7B {
            extra_sid_addrs[1] = decode_sid_addr_byte(data[0x7B]);
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
        extra_sid_addrs,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
//  C64 memory bus – intercepts SID writes
// ─────────────────────────────────────────────────────────────────────────────

struct C64Memory {
    ram: [u8; 65536],
    /// Collected SID writes: (usbsid_register, value).
    pub sid_writes: Vec<(u8, u8)>,
    mapper: SidMapper,
    /// True if running in mono mode (no multi-SID routing).
    mono: bool,
}

impl C64Memory {
    fn new(is_pal: bool, mapper: SidMapper, mono: bool) -> Self {
        let mut ram = [0u8; 65536];
        ram[0x0001] = 0x37;
        ram[0x02A6] = if is_pal { 0x01 } else { 0x00 };
        Self {
            ram,
            sid_writes: Vec::with_capacity(256),
            mapper,
            mono,
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

        if self.mono {
            // Mono: all $D400–$D7FF → SID1 (mirrors fold every 32 bytes)
            if (0xD400..=0xD7FF).contains(&address) {
                self.sid_writes.push(((address as u8) & 0x1F, value));
            }
        } else {
            // Multi-SID: route through mapper
            if let Some(reg) = self.mapper.map(address) {
                self.sid_writes.push((reg, value));
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  6502 runner
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
//  CLI helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Parse a hex address from "$D440", "0xD440", or "D440".
fn parse_hex_addr(s: &str) -> Option<u16> {
    let s = s.trim();
    let hex = if let Some(h) = s.strip_prefix("$") {
        h
    } else if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        h
    } else {
        s
    };
    u16::from_str_radix(hex, 16).ok()
}

// ─────────────────────────────────────────────────────────────────────────────
//  Main
// ─────────────────────────────────────────────────────────────────────────────

fn main() {
    env_logger::init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <file.sid> [song_number] [flags]", args[0]);
        eprintln!();
        eprintln!("Flags:");
        eprintln!("  --stereo       Mirror mono SID1 to SID2 (both speakers)");
        eprintln!("  --sid4 <addr>  4th SID at C64 address (e.g. --sid4 $D440)");
        eprintln!();
        eprintln!("2SID/3SID files are auto-detected from the v3/v4 header.");
        process::exit(1);
    }

    let force_stereo = args.iter().any(|a| a == "--stereo");

    // Parse --sid4 address
    let sid4_addr: u16 = args
        .windows(2)
        .find(|w| w[0] == "--sid4")
        .and_then(|w| parse_hex_addr(&w[1]))
        .unwrap_or(0);

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
        .find(|a| !a.starts_with("--") && a.parse::<u16>().is_ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(header.start_song);

    // ── Build SID address list ───────────────────────────────────────────
    // SID1 is always $D400. Extra SIDs come from header + CLI.
    let mut sid_bases: Vec<u16> = vec![0xD400];

    if header.extra_sid_addrs[0] != 0 {
        sid_bases.push(header.extra_sid_addrs[0]);
    }
    if header.extra_sid_addrs[1] != 0 {
        sid_bases.push(header.extra_sid_addrs[1]);
    }
    if sid4_addr != 0 {
        sid_bases.push(sid4_addr);
    }

    let num_sids = sid_bases.len();
    let is_multi = num_sids > 1;
    let use_stereo = is_multi || force_stereo;

    // If --stereo on mono tune, we mirror SID1→SID2 but still only capture
    // SID1 writes from the bus (mono=true).
    let mono_mode = !is_multi;

    let mapper = SidMapper::new(&sid_bases);

    // ── Display ──────────────────────────────────────────────────────────
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

    for (i, &base) in sid_bases.iter().enumerate() {
        let usb_base = (i as u8) * SID_REG_SIZE;
        println!(
            "│  SID{}   : ${:04X} → USBSID ${:02X}–${:02X}",
            i + 1,
            base,
            usb_base,
            usb_base + 0x1F
        );
    }

    println!(
        "│  Output : {}",
        match (num_sids, force_stereo) {
            (1, false) => "MONO (1 SID)",
            (1, true) => "STEREO (mono mirrored to SID2)",
            (2, _) => "STEREO (2SID)",
            (3, _) => "3SID",
            (4.., _) => "4SID",
            _ => "MULTI-SID",
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
    let mut mem = C64Memory::new(header.is_pal, mapper, mono_mode);
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

    // Enable stereo if using more than 1 SID
    if use_stereo {
        usbsid.set_stereo(1);
    } else {
        usbsid.set_stereo(0);
    }

    // Max volume on all SIDs
    let active_sids = if use_stereo && mono_mode { 2 } else { num_sids };
    for i in 0..active_sids {
        let vol_reg = (i as u8) * SID_REG_SIZE + SID_VOL_REG;
        let _ = usbsid.write(vol_reg, 0x0F);
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

    let mirror_mono = force_stereo && mono_mode;

    println!("  Playing... (Ctrl+C to stop)\n");

    while running.load(Ordering::Relaxed) {
        let t = Instant::now();

        cpu.registers.program_counter = trampoline;
        cpu.registers.stack_pointer = StackPointer(0xFD);
        cpu.memory.clear_writes();

        run_until(&mut cpu, halt_pc, 200_000);

        if mirror_mono {
            // Mono mirror: duplicate SID1 writes (regs $00–$18) to SID2 ($20–$38)
            let mut all: Vec<(u8, u8)> = Vec::with_capacity(cpu.memory.sid_writes.len() * 2);
            for &(reg, val) in &cpu.memory.sid_writes {
                all.push((reg, val));
                if reg <= SID_VOL_REG {
                    all.push((reg + SID_REG_SIZE, val));
                }
            }
            flush_writes(&mut usbsid, &all);
        } else {
            // Multi-SID or plain mono: mapper already set correct register bytes
            flush_writes(&mut usbsid, &cpu.memory.sid_writes);
        }

        let secs = t0.elapsed().as_secs();
        print!(
            "\r  ▶ {:02}:{:02}  {} writes",
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
