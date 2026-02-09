/*
 * USBSID-Pico Rust driver – C-compatible header

 * Build the Rust crate as a C-dynamic or static library:
 *   cargo build --release
 *
 * Then link against target/release/libusbsid_pico.so (or .dylib / .lib).
 *
 */

#ifndef _USBSID_PICO_H_
#define _USBSID_PICO_H_

#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque handle to the Rust UsbSid object */
typedef void *USBSIDitf;

/* ── Lifecycle ──────────────────────────────────────────────────────────── */
USBSIDitf     create_USBSID(void);
int           init_USBSID(USBSIDitf p, bool start_threaded, bool with_cycles);
void          close_USBSID(USBSIDitf p);

/* ── SID / device control ───────────────────────────────────────────────── */
void          pause_USBSID(USBSIDitf p);
void          reset_USBSID(USBSIDitf p);
void          resetallregisters_USBSID(USBSIDitf p);
void          clearbus_USBSID(USBSIDitf p);
void          mute_USBSID(USBSIDitf p);
void          unmute_USBSID(USBSIDitf p);

/* ── Clock rate ─────────────────────────────────────────────────────────── */
void          setclockrate_USBSID(USBSIDitf p, int64_t clockrate_cycles, bool suspend_sids);
int64_t       getclockrate_USBSID(USBSIDitf p);
int64_t       getrefreshrate_USBSID(USBSIDitf p);
int64_t       getrasterrate_USBSID(USBSIDitf p);

/* ── Device info ────────────────────────────────────────────────────────── */
int           getnumsids_USBSID(USBSIDitf p);
int           getfmoplsid_USBSID(USBSIDitf p);
int           getpcbversion_USBSID(USBSIDitf p);
void          setstereo_USBSID(USBSIDitf p, int state);
void          togglestereo_USBSID(USBSIDitf p);

/* ── Status helpers ─────────────────────────────────────────────────────── */
bool          initialised_USBSID(USBSIDitf p);
bool          available_USBSID(USBSIDitf p);
bool          portisopen_USBSID(USBSIDitf p);

/* ── Synchronous I/O ────────────────────────────────────────────────────── */
void          writesingle_USBSID(USBSIDitf p, const unsigned char *buff, int len);
unsigned char readsingle_USBSID(USBSIDitf p, uint8_t reg);

/* ── Asynchronous direct I/O ────────────────────────────────────────────── */
void          writebuffer_USBSID(USBSIDitf p, const unsigned char *buff, int len);
void          write_USBSID(USBSIDitf p, uint8_t reg, uint8_t val);
void          writecycled_USBSID(USBSIDitf p, uint8_t reg, uint8_t val, uint16_t cycles);
unsigned char read_USBSID(USBSIDitf p, uint8_t reg);

/* ── Ring-buffer writes (threaded mode) ─────────────────────────────────── */
void          writering_USBSID(USBSIDitf p, uint8_t reg, uint8_t val);
void          writeringcycled_USBSID(USBSIDitf p, uint8_t reg, uint8_t val, uint16_t cycles);

/* ── Thread / ring-buffer management ────────────────────────────────────── */
void          enablethread_USBSID(USBSIDitf p);
void          disablethread_USBSID(USBSIDitf p);
void          setflush_USBSID(USBSIDitf p);
void          flush_USBSID(USBSIDitf p);
void          restartringbuffer_USBSID(USBSIDitf p);
void          setbuffsize_USBSID(USBSIDitf p, int size);
void          setdiffsize_USBSID(USBSIDitf p, int size);
void          restartthread_USBSID(USBSIDitf p, bool with_cycles);

/* ── Timing ─────────────────────────────────────────────────────────────── */
int64_t       waitforcycle_USBSID(USBSIDitf p, uint64_t cycles);

#ifdef __cplusplus
}
#endif

#endif /* _USBSID_PICO_H_ */