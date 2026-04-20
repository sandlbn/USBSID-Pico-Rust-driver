// USBSID-Pico is a RPi Pico (RP2040/RP2350) based board for interfacing one
// or two MOS SID chips and/or hardware SID emulators over (WEB)USB with your
// computer, phone or ASID supporting player.
//
// Copyright (c) 2024-2025 LouD
// Licensed under GPL-2.0

//! Fixed-size, single-producer / single-consumer ring buffer used by the
//! background writer thread.
//!
//! Uses atomic cursors so the producer (player thread) can push data without
//! acquiring the shared mutex — matching the C++ driver's lock-free RingPut.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::constants::{DEFAULT_DIFF_SIZE, DEFAULT_RING_SIZE, MIN_DIFF_SIZE, MIN_RING_SIZE};

/// A byte-level SPSC ring buffer with lock-free put/get.
///
/// The producer calls [`put`] without any external lock.
/// The consumer calls [`get`] without any external lock.
/// Safety relies on single-producer / single-consumer discipline.
pub struct RingBuffer {
    /// Backing storage — UnsafeCell allows mutation from &self.
    buf: UnsafeCell<Vec<u8>>,
    /// Current read cursor (only mutated by consumer).
    read_pos: AtomicUsize,
    /// Current write cursor (only mutated by producer).
    write_pos: AtomicUsize,
    /// Total ring capacity.
    ring_size: usize,
    /// Minimum difference between head and tail before the writer thread pops.
    diff_size: usize,
}

// SAFETY: RingBuffer is designed for single-producer / single-consumer use.
// The backing Vec is allocated once and never reallocated during SPSC operation.
// read_pos is only mutated by the consumer, write_pos only by the producer.
// Producer writes to buf[write_pos], consumer reads from buf[read_pos].
// These never overlap because is_full() prevents write_pos from catching read_pos.
unsafe impl Send for RingBuffer {}
unsafe impl Sync for RingBuffer {}

impl RingBuffer {
    /// Create a new ring buffer with the given capacity and diff threshold.
    pub fn new(ring_size: usize, diff_size: usize) -> Self {
        let ring_size = ring_size.max(MIN_RING_SIZE);
        let diff_size = diff_size.max(MIN_DIFF_SIZE);
        Self {
            buf: UnsafeCell::new(vec![0u8; ring_size]),
            read_pos: AtomicUsize::new(0),
            write_pos: AtomicUsize::new(0),
            ring_size,
            diff_size,
        }
    }

    /// Create with default sizes.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_RING_SIZE, DEFAULT_DIFF_SIZE)
    }

    // ── Capacity / state ─────────────────────────────────────────────────

    /// Current ring capacity.
    pub fn capacity(&self) -> usize {
        self.ring_size
    }

    /// Current diff-size threshold.
    pub fn diff_threshold(&self) -> usize {
        self.diff_size
    }

    /// Returns `true` when there is data to read and the distance between
    /// read and write cursors exceeds `diff_size`.
    pub fn has_data(&self) -> bool {
        let r = self.read_pos.load(Ordering::Acquire);
        let w = self.write_pos.load(Ordering::Acquire);
        r != w && r.abs_diff(w) > self.diff_size
    }

    /// Returns `true` when read and write cursors are equal.
    pub fn is_empty(&self) -> bool {
        self.read_pos.load(Ordering::Acquire) == self.write_pos.load(Ordering::Acquire)
    }

    /// Absolute distance between read and write cursors.
    pub fn diff(&self) -> usize {
        self.read_pos
            .load(Ordering::Acquire)
            .abs_diff(self.write_pos.load(Ordering::Acquire))
    }

    // ── Lock-free mutation ───────────────────────────────────────────────

    /// Returns `true` when the ring buffer is full.
    pub fn is_full(&self) -> bool {
        let w = self.write_pos.load(Ordering::Relaxed);
        let r = self.read_pos.load(Ordering::Acquire);
        (w + 1) % self.ring_size == r
    }

    /// Push one byte into the ring, advancing the write cursor.
    /// Lock-free — safe to call from the producer without any mutex.
    /// Returns `false` if the ring is full and the byte was dropped.
    pub fn put(&self, item: u8) -> bool {
        let w = self.write_pos.load(Ordering::Relaxed);
        let r = self.read_pos.load(Ordering::Acquire);
        if (w + 1) % self.ring_size == r {
            return false; // full
        }
        // SAFETY: only the producer calls put(), so no concurrent write to buf[w].
        // The consumer only reads at read_pos, which is behind write_pos.
        unsafe { (&mut (*self.buf.get()))[w] = item };
        self.write_pos
            .store((w + 1) % self.ring_size, Ordering::Release);
        true
    }

    /// Pop one byte from the ring, advancing the read cursor.
    /// Lock-free — safe to call from the consumer without any mutex.
    pub fn get(&self) -> u8 {
        let r = self.read_pos.load(Ordering::Relaxed);
        // SAFETY: only the consumer calls get(), and the producer never
        // writes to positions at or behind read_pos.
        let item = unsafe { (&(*self.buf.get()))[r] };
        self.read_pos
            .store((r + 1) % self.ring_size, Ordering::Release);
        item
    }

    /// Reset both cursors to zero.
    pub fn reset(&self) {
        self.read_pos.store(0, Ordering::Release);
        self.write_pos.store(0, Ordering::Release);
    }

    // ── Reconfiguration (NOT thread-safe — call only when stopped) ──────

    /// Update the ring capacity (reallocates). Resets cursors.
    pub fn set_ring_size(&mut self, size: usize) {
        self.ring_size = size.max(MIN_RING_SIZE);
        self.buf.get_mut().resize(self.ring_size, 0);
        self.reset();
    }

    /// Update the diff threshold.
    pub fn set_diff_size(&mut self, size: usize) {
        self.diff_size = size.max(MIN_DIFF_SIZE);
    }

    /// Full re-initialisation: new capacity + diff, cursors reset.
    pub fn reinit(&mut self, ring_size: usize, diff_size: usize) {
        self.set_ring_size(ring_size);
        self.set_diff_size(diff_size);
    }

    /// Reset to defaults.
    pub fn reinit_defaults(&mut self) {
        self.reinit(DEFAULT_RING_SIZE, DEFAULT_DIFF_SIZE);
    }
}
