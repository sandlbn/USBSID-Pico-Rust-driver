# usbsid-pico

Rust driver for the [USBSID-Pico](https://github.com/LouDnl/USBSID-Pico) — a USB interface for real MOS SID chips (6581/8580).

## Requirements

- Rust 1.70+
- `libusb-1.0` (`apt install libusb-1.0-0-dev` on Linux, `brew install libusb` on macOS)
- A USBSID-Pico device connected via USB

### Linux USB permissions

```bash
sudo tee /etc/udev/rules.d/99-usbsid-pico.rules << 'EOF'
SUBSYSTEM=="usb", ATTR{idVendor}=="cafe", ATTR{idProduct}=="4011", MODE="0666"
EOF
sudo udevadm control --reload-rules && sudo udevadm trigger
```

## Build

```bash
cargo build --release
```

## Examples

**Direct register writes** — plays an arpeggio, no .sid file needed:
```bash
cargo run --example simple_tone
```

**SID file player** — plays .sid files from the [High Voltage SID Collection](https://hvsc.c64.org):
```bash
cargo run --example sid_player -- path/to/tune.sid
```

**API smoke test:**
```bash
cargo run --example basic
```

## Usage as a library

```rust
use usbsid_pico::{UsbSid, ClockSpeed};

let mut sid = UsbSid::new();
sid.init(false, false).expect("USB connection failed");
sid.set_clock_rate(ClockSpeed::Pal as i64, true);

// Write to SID register (e.g. set max volume)
sid.write(0x18, 0x0F);

// Cleanup
sid.mute();
sid.reset();
sid.close();
```

## C FFI

Build as a shared library for use from C/C++:

```bash
cargo build --release
```

Link against `libusbsid_pico.so` / `.dylib` / `.dll` and use the header `usbsid_pico.h`.
