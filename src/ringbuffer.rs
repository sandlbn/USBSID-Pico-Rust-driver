// USBSID-Pico is a RPi Pico (RP2040/RP2350) based board for interfacing one
// or two MOS SID chips and/or hardware SID emulators over (WEB)USB with your
// computer, phone or ASID supporting player.
//
// Copyright (c) 2024-2025 LouD
// Licensed under GPL-2.0

//! Fixed-size, single-producer / single-consumer ring buffer used by the
//! background writer thread.

use crate::constants::{DEFAULT_DIFF_SIZE, DEFAULT_RING_SIZE, MIN_DIFF_SIZE, MIN_RING_SIZE};

/// A byte-level ring buffer with configurable capacity.
pub struct RingBuffer {
    /// Backing storage.
    buf: Vec<u8>,
    /// Current read cursor.
    pub read_pos: usize,
    /// Current write cursor.
    pub write_pos: usize,
    /// Total ring capacity.
    ring_size: usize,
    /// Minimum difference between head and tail before the writer thread pops.
    diff_size: usize,
}

impl RingBuffer {
    /// Create a new ring buffer with the given capacity and diff threshold.
    pub fn new(ring_size: usize, diff_size: usize) -> Self {
        let ring_size = ring_size.max(MIN_RING_SIZE);
        let diff_size = diff_size.max(MIN_DIFF_SIZE);
        Self {
            buf: vec![0u8; ring_size],
            read_pos: 0,
            write_pos: 0,
            ring_size,
            diff_size,
        }
    }

    /// Create with default sizes (8192 / 64).
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
        self.read_pos != self.write_pos && self.diff() > self.diff_size
    }

    /// Returns `true` when read and write cursors differ.
    pub fn is_empty(&self) -> bool {
        self.read_pos == self.write_pos
    }

    /// Absolute distance between read and write cursors.
    pub fn diff(&self) -> usize {
        if self.read_pos < self.write_pos {
            self.write_pos - self.read_pos
        } else {
            self.read_pos - self.write_pos
        }
    }

    // ── Mutation ──────────────────────────────────────────────────────────

    /// Push one byte into the ring, advancing the write cursor.
    pub fn put(&mut self, item: u8) {
        self.buf[self.write_pos] = item;
        self.write_pos = (self.write_pos + 1) % self.ring_size;
    }

    /// Pop one byte from the ring, advancing the read cursor.
    pub fn get(&mut self) -> u8 {
        let item = self.buf[self.read_pos];
        self.read_pos = (self.read_pos + 1) % self.ring_size;
        item
    }

    /// Reset both cursors to zero.
    pub fn reset(&mut self) {
        self.read_pos = 0;
        self.write_pos = 0;
    }

    // ── Reconfiguration ──────────────────────────────────────────────────

    /// Update the ring capacity (reallocates).  Resets cursors.
    pub fn set_ring_size(&mut self, size: usize) {
        self.ring_size = size.max(MIN_RING_SIZE);
        self.buf.resize(self.ring_size, 0);
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

    /// Reset to defaults (capacity 8192, diff 64).
    pub fn reinit_defaults(&mut self) {
        self.reinit(DEFAULT_RING_SIZE, DEFAULT_DIFF_SIZE);
    }
}
