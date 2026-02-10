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
- **libusb 1.0** development headers (not needed when using the `serial` feature):

| Platform | Install |
|----------|---------|
| Debian/Ubuntu | `sudo apt install libusb-1.0-0-dev` |
| Fedora | `sudo dnf install libusb1-devel` |
| macOS | `brew install libusb` |
| Windows | Not needed with `--features serial` (recommended) |

## Platform notes

### Linux

Works out of the box with libusb. You may need a udev rule for non-root access:

```bash
echo 'SUBSYSTEM=="usb", ATTR{idVendor}=="cafe", ATTR{idProduct}=="4011", MODE="0666"' | \
  sudo tee /etc/udev/rules.d/99-usbsid.rules
sudo udevadm control --reload-rules
```

### macOS

Requires `brew install libusb` and may need `sudo` for USB access.
See [SIGNING.md](SIGNING.md) for code signing to avoid `sudo`.

### Windows

libusb on Windows requires a WinUSB driver (via [Zadig](https://zadig.akeo.ie/)),
which replaces the default COM port driver. To avoid this, use the **serial backend**
instead — it talks to the USBSID-Pico through the COM port that Windows assigns
automatically, with no driver changes needed:

```bash
cargo build --features serial
```

When `serial` is enabled, the driver tries libusb first and automatically falls back
to the serial port if libusb fails. This means the same binary works on all platforms.

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

# Play a .sid file (2SID/3SID auto-detected from header)
cargo run --example sid_player -- path/to/tune.sid

# Mirror mono tune to both SID chips
cargo run --example sid_player -- path/to/tune.sid --stereo

# 4SID with manual SID4 address
cargo run --example sid_player -- path/to/tune.sid --sid4 $DE00
```

On Windows, add `--features serial` to any of the above:

```bash
cargo run --features serial --example sid_player -- path/to/tune.sid
```

## Cargo features

| Feature | Default | Description |
|---------|---------|-------------|
| `serial` | No | Enables serial port backend (requires `serialport` crate). Recommended for Windows. Also works on macOS (`/dev/tty.usbmodem*`) and Linux (`/dev/ttyACM*`). |
| `debug_memory` | No | Enable SID memory tracking for debugging. |

## Architecture

| Module | Description |
|--------|-------------|
| `constants` | Protocol opcodes, USB IDs, clock/timing tables, SID address helpers |
| `device` | Core `UsbSid` struct — USB setup, I/O, threading, timing |
| `transport` | Transport abstraction: libusb and serial port backends |
| `ringbuffer` | Lock-free SPSC ring buffer for the writer thread |
| `error` | `UsbSidError` enum and `Result` alias |
| `ffi` | `extern "C"` functions for C/C++ consumers |

### Transport backends

The driver abstracts I/O through a `Transport` trait with two implementations:

| Backend | When used | Dependency |
|---------|-----------|------------|
| **USB** (libusb) | Default on all platforms | `rusb` (always included) |
| **Serial** (COM port) | Fallback when libusb fails and `serial` feature is enabled | `serialport` (optional) |

The `init()` method tries libusb first. If it fails and the `serial` feature is compiled in,
it automatically scans for a USBSID-Pico on available serial ports (matching VID `0xCAFE` /
PID `0x4011`) and connects through the COM port. No code changes needed — just enable the feature.

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
