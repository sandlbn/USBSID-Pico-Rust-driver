//! Run with:
//!   cargo run --example basic
//!
//! (Requires a USBSID-Pico device to be connected.)

use usbsid_pico::UsbSid;

fn main() {
    // Initialise env_logger so debug!() / error!() output is visible.
    // Set RUST_LOG=debug for full trace.
    env_logger::init();

    // ── Create & init ────────────────────────────────────────────────────
    let mut us = UsbSid::new();

    if let Err(e) = us.init(true, true) {
        eprintln!("Failed to initialise USBSID: {e}");
        return;
    }

    let is_init = us.is_initialised();
    let is_avail = us.is_available();
    let is_open = us.is_open();
    println!("initialised={is_init}  available={is_avail}  open={is_open}");

    // ── Synchronous I/O ──────────────────────────────────────────────────
    let buf = [0u8; 4];
    let _ = us.single_write(&buf);
    let rs = us.single_read(0x0C).unwrap_or(0);
    println!("single_read(0x0C) = 0x{rs:02X}");

    // ── Asynchronous direct I/O (only works in non-threaded mode) ────────
    us.disable_thread();

    let _ = us.write_buffer(&buf);
    let _ = us.write(0x01, 0x01);
    let _ = us.write_cycled(0x01, 0x01, 0xFFFF);
    let r = us.read(0x0C).unwrap_or(0);
    println!("read(0x0C) = 0x{r:02X}");

    // ── Re-enable threaded mode for ring-buffer writes ───────────────────
    let _ = us.enable_thread();

    let _ = us.write_ring_cycled(0x01, 0x01, 0xFFFF);

    // ── Thread / ring-buffer management ──────────────────────────────────
    let _ = us.enable_thread();
    us.disable_thread();
    us.set_flush();
    us.flush();
    us.restart_ring_buffer();
    us.set_buffer_size(8192);
    us.set_diff_size(64);
    let _ = us.restart_thread(true);

    // ── Timing ───────────────────────────────────────────────────────────
    let d = us.wait_for_cycle(0xFFFF);
    println!("wait_for_cycle(0xFFFF) = {d} ns");

    // ── Clock / device info ──────────────────────────────────────────────
    us.set_clock_rate(985_248, true); // PAL
    let cr = us.get_clock_rate();
    let rr = us.get_refresh_rate();
    let rar = us.get_raster_rate();
    let nsids = us.get_num_sids();
    let nfmsid = us.get_fmopl_sid();
    let pcbv = us.get_pcb_version();
    println!(
        "clock={cr}  refresh={rr}  raster={rar}  sids={nsids}  fmopl={nfmsid}  pcb_ver={pcbv}"
    );

    // ── Stereo ───────────────────────────────────────────────────────────
    us.set_stereo(1);
    us.toggle_stereo();

    // ── Control commands ─────────────────────────────────────────────────
    us.pause();
    us.mute();
    us.unmute();
    us.clear_bus();
    us.reset_all_registers();
    us.reset();

    // ── Cleanup (also happens automatically via Drop) ────────────────────
    us.close();
    println!("Done.");
}
