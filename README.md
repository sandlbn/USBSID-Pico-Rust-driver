# usbsid-pico

[![Crates.io](https://img.shields.io/crates/v/usbsid-pico.svg)](https://crates.io/crates/usbsid-pico)
[![Documentation](https://docs.rs/usbsid-pico/badge.svg)](https://docs.rs/usbsid-pico)
[![License](https://img.shields.io/crates/l/usbsid-pico.svg)](LICENSE-MIT)

Rust driver for the **[USBSID-Pico](https://github.com/LouDnl/USBSID-Pico)** board — a
Raspberry Pi Pico (RP2040 / RP2350) based device for interfacing one or more
MOS SID chips (6581/8580) and hardware SID emulators over USB.

## Features

- Synchronous and asynchronous (threaded) write modes
- Ring-buffer backed background writer for low-latency streaming
- Cycle-accurate writes for emulator integration
- Up to 4 SID chips (stereo / 3SID / 4SID)
- Clock rate configuration (PAL / NTSC)
- C FFI layer for integration with existing C/C++ applications
- Cross-platform: Linux, macOS, Windows

## Requirements

- **Rust 1.70+** (2021 edition)
- **libusb 1.0** development headers:

| Platform | Install |
|----------|---------|
| Debian/Ubuntu | `sudo apt install libusb-1.0-0-dev` |
| Fedora | `sudo dnf install libusb1-devel` |
| macOS | `brew install libusb` |
| Windows | [vcpkg](https://github.com/microsoft/vcpkg) or pre-built libs |

## Quick start

```rust,no_run
use usbsid_pico::UsbSid;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sid = UsbSid::new();
    sid.init(/* threaded */ true, /* with_cycles */ true)?;

    // Write to SID register via the ring buffer
    sid.write_ring_cycled(0x01, 0x01, 0xFFFF)?;

    // Read a register (synchronous)
    let val = sid.single_read(0x1B)?;
    println!("OSC3 random: 0x{:02X}", val);

    // Automatically closed on drop
    Ok(())
}
```

## Examples

Run with a connected USBSID-Pico:

```bash
# Simple register test
cargo run --example basic

# Generate a tone
cargo run --example simple_tone

# Play a .sid file (mono)
cargo run --example sid_player -- path/to/tune.sid

# Play a .sid file (2SID/3SID auto-detected, or force stereo mirror)
cargo run --example sid_player -- path/to/tune.sid --stereo
```

On macOS you may need `sudo` for USB access (see [SIGNING.md](SIGNING.md)).

## Architecture

| Module | Description |
|--------|-------------|
| `constants` | Protocol opcodes, USB IDs, clock/timing tables, SID address helpers |
| `device` | Core `UsbSid` struct — USB setup, I/O, threading, timing |
| `ringbuffer` | Lock-free SPSC ring buffer for the writer thread |
| `error` | `UsbSidError` enum and `Result` alias |
| `ffi` | `extern "C"` functions for C/C++ consumers |

### Write modes

| Mode | Function | Use case |
|------|----------|----------|
| Synchronous | `single_write` / `single_read` | Direct bulk transfers |
| Async direct | `write` / `write_cycled` | Non-threaded bulk writes |
| Async threaded | `write_ring` / `write_ring_cycled` | Background thread drains ring buffer |

### USBSID-Pico register layout

Each SID chip occupies 32 registers (`0x20` bytes):

| SID | USBSID registers |
|-----|-----------------|
| SID1 | `$00–$1F` |
| SID2 | `$20–$3F` |
| SID3 | `$40–$5F` |
| SID4 | `$60–$7F` |

## C FFI

The crate exposes a C-compatible interface. Build as a shared library:

```toml
[lib]
crate-type = ["cdylib", "rlib"]
```

```bash
cargo build --release
# → target/release/libusbsid_pico.{so,dylib,dll}
```

To auto-generate the C header:

```bash
cargo install cbindgen
cbindgen --config cbindgen.toml --crate usbsid-pico --output usbsid_pico.h
```

## License

Licensed under either of

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.

## Acknowledgments

The [USBSID-Pico](https://github.com/LouDnl/USBSID-Pico) hardware and firmware
are created by [LouDnl](https://github.com/LouDnl). This driver is an independent
implementation targeting the USBSID-Pico USB protocol.
