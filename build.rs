// USBSID-Pico build script
//
// This build script:
//   1. Emits linker flags for libusb-1.0 (used transitively by `rusb`).
//   2. Optionally generates a C header (`usbsid_pico.h`) via `cbindgen`
//      so that C/C++ projects can call the Rust FFI functions.
//
// To generate the C header during development, install cbindgen:
//   cargo install cbindgen
//
// Then run:
//   cbindgen --config cbindgen.toml --crate usbsid-pico --output usbsid_pico.h
//
// Or enable the header generation below (requires cbindgen as a build-dependency).

fn main() {
    // ── Link libraries ───────────────────────────────────────────────────

    // `rusb` already links libusb via its own build script, but if you
    // build this crate as a cdylib/staticlib for C consumers you may need
    // pthread explicitly on Linux, or framework links on macOS.
    #[cfg(target_os = "linux")]
    println!("cargo:rustc-link-lib=pthread");

    // On macOS, libusb depends on IOKit and CoreFoundation frameworks.
    // rusb's build script handles this, but if linking as a static lib
    // for C consumers these may be needed.
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-lib=framework=IOKit");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=Security");
    }

    // ── (Optional) Auto-generate C header with cbindgen ──────────────────

    // Please uncomment the block below if you add `cbindgen` to [build-dependencies]
    // in Cargo.toml and want the header generated every build.
    //
    // ```toml
    // [build-dependencies]
    // cbindgen = "0.27"
    // ```
    //
    // let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    // cbindgen::Builder::new()
    //     .with_crate(crate_dir)
    //     .with_language(cbindgen::Language::C)
    //     .with_include_guard("_USBSID_PICO_H_")
    //     .generate()
    //     .expect("Unable to generate C bindings")
    //     .write_to_file("usbsid_pico.h");
}
