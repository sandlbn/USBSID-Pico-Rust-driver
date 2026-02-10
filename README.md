# usbsid-pico

[![Crates.io](https://img.shields.io/crates/v/usbsid-pico.svg)](https://crates.io/crates/usbsid-pico)
[![Documentation](https://docs.rs/usbsid-pico/badge.svg)](https://docs.rs/usbsid-pico)
[![License](https://img.shields.io/crates/l/usbsid-pico.svg)](LICENSE-MIT)

Rust driver for the **[USBSID-Pico](https://github.com/LouDnl/USBSID-Pico)** — a
Raspberry Pi Pico (RP2040 / RP2350) based board for interfacing one or more
MOS SID chips (6581/8580) and hardware SID emulators over USB.

This is a Rust implementation of the [original C++ driver](https://github.com/LouDnl/USBSID-Pico-driver)
by [LouDnl](https://github.com/LouDnl). The original driver uses libusb;
this crate defaults to serial port communication instead, which avoids
platform-specific USB driver setup.

## Requirements

- Rust 1.70+ (2021 edition)
- A connected USBSID-Pico device
- On Linux: `libudev-dev` for serial port enumeration
  ```bash
  sudo apt install libudev-dev   # Debian/Ubuntu
  sudo dnf install systemd-devel # Fedora
  ```

## Building

```bash
cargo build --release
```

The default `serial` feature uses the OS-provided COM / tty port — no
additional drivers or libraries are needed on any platform.

If you want the libusb backend instead (matching the original C++ driver),
enable the `usb` feature. This requires libusb 1.0 headers and on Windows
a WinUSB driver via [Zadig](https://zadig.akeo.ie/):

```bash
cargo build --features usb
```

When both features are enabled, the driver tries USB first and falls back
to serial.

## Quick start

```rust,no_run
use usbsid_pico::UsbSid;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sid = UsbSid::new();
    sid.init(/* threaded */ true, /* with_cycles */ true)?;

    // Write to SID register via the ring buffer
    sid.write_ring_cycled(0x01, 0x01, 0xFFFF)?;

    // Read a register
    let val = sid.single_read(0x1B)?;
    println!("OSC3 random: 0x{:02X}", val);

    Ok(())
}
```

## Examples

```bash
cargo run --example basic
cargo run --example simple_tone
cargo run --example sid_player -- path/to/tune.sid
cargo run --example sid_player -- path/to/tune.sid --stereo
cargo run --example sid_player -- path/to/tune.sid --sid4 $DE00
```

The `sid_player` example includes a 6502 CPU emulator and supports
PSID/RSID v1–v4 files with automatic multi-SID detection from the header.

## Features

| Feature | Default | Description |
|---------|---------|-------------|
| `serial` | Yes | Serial port backend via `serialport`. No driver setup needed. |
| `usb` | No | libusb backend via `rusb`. Matches original C++ driver. Needs libusb headers and on Windows a WinUSB driver. |
| `debug_memory` | No | SID memory tracking. |

## Architecture

| Module | Description |
|--------|-------------|
| `device` | Core `UsbSid` struct — connection setup, I/O, threading, timing |
| `transport` | Transport trait with serial and libusb backends |
| `constants` | Protocol opcodes, USB IDs, clock/timing tables, SID address helpers |
| `ringbuffer` | Lock-free SPSC ring buffer for the background writer thread |
| `error` | `UsbSidError` enum and `Result` alias |
| `ffi` | `extern "C"` functions for C/C++ consumers |

### Write modes

| Mode | Function | Description |
|------|----------|-------------|
| Synchronous | `single_write` / `single_read` | Blocking transfers |
| Async direct | `write` / `write_cycled` | Non-threaded writes |
| Async threaded | `write_ring` / `write_ring_cycled` | Background thread drains ring buffer |

### SID register layout

Each SID occupies 32 registers (`0x20` bytes):

| SID | Registers |
|-----|-----------|
| SID1 | `$00–$1F` |
| SID2 | `$20–$3F` |
| SID3 | `$40–$5F` |
| SID4 | `$60–$7F` |

## C FFI

The crate exposes a C-compatible interface. Build as a shared library:

```bash
cargo build --release
# → target/release/libusbsid_pico.{so,dylib,dll}
```

Generate the C header with cbindgen:

```bash
cargo install cbindgen
cbindgen --config cbindgen.toml --crate usbsid-pico --output usbsid_pico.h
```

## Platform notes

**Linux** — You may need a udev rule for non-root access:

```bash
echo 'SUBSYSTEM=="usb", ATTR{idVendor}=="cafe", ATTR{idProduct}=="4011", MODE="0666"' | \
  sudo tee /etc/udev/rules.d/99-usbsid.rules
sudo udevadm control --reload-rules
```

**macOS** — With the default serial backend no special setup is needed.

**Windows** — The default serial backend uses the COM port that Windows
assigns automatically. If using the `usb` feature you need to install a
WinUSB driver with [Zadig](https://zadig.akeo.ie/).

## License

Licensed under either of

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

at your option.

## Acknowledgments

[USBSID-Pico](https://github.com/LouDnl/USBSID-Pico) hardware and firmware
by [LouDnl](https://github.com/LouDnl).