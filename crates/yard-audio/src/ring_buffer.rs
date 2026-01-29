//! Lock-free audio ring buffer for efficient inter-thread audio transfer.
//!
//! This module provides an `AudioRingBuffer` that allows a producer (network thread)
//! to write audio data while a consumer (PipeWire callback) reads it, without
//! blocking locks.
//!
//! # Design
//!
//! The ring buffer uses atomic operations for head/tail pointers, enabling
//! lock-free Single Producer Single Consumer (SPSC) operation. The buffer
//! capacity is always a power of 2 for efficient modulo operations.
//!
//! # Example
//!
//! ```ignore
//! use yard_audio::AudioRingBuffer;
//!
//! let mut buffer = AudioRingBuffer::new(65536); // 64KB buffer
//!
//! // Producer writes data
//! let written = buffer.push(&audio_data);
//!
//! // Consumer reads data
//! let mut output = [0u8; 1024];
//! let read = buffer.pop(&mut output);
//! ```

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Default buffer capacity: 64KB (~200ms at 48kHz stereo 16-bit).
pub const DEFAULT_BUFFER_CAPACITY: usize = 65536;

/// Statistics for monitoring ring buffer performance.
#[derive(Debug, Clone, Copy, Default)]
pub struct RingBufferStats {
    /// Number of times the buffer overflowed (data dropped).
    pub overflows: u64,
    /// Number of times the buffer underran (silence played).
    pub underruns: u64,
    /// Total bytes written to the buffer.
    pub bytes_written: u64,
    /// Total bytes read from the buffer.
    pub bytes_read: u64,
    /// Current fill level in bytes.
    pub current_fill: usize,
    /// Buffer capacity in bytes.
    pub capacity: usize,
}

impl RingBufferStats {
    /// Returns the fill percentage (0.0 to 1.0).
    #[must_use]
    pub fn fill_percentage(&self) -> f32 {
        if self.capacity == 0 {
            return 0.0;
        }
        self.current_fill as f32 / self.capacity as f32
    }
}

/// A lock-free ring buffer for audio data transfer between threads.
///
/// Uses atomic operations for head/tail pointers to enable safe concurrent
/// access from a single producer and single consumer without locks.
///
/// # Thread Safety
///
/// This buffer is designed for Single Producer Single Consumer (SPSC) use:
/// - One thread calls `push()` (producer)
/// - One thread calls `pop()` (consumer)
///
/// Multiple producers or consumers require external synchronization.
pub struct AudioRingBuffer {
    /// The underlying buffer storage.
    buffer: Box<[u8]>,
    /// Buffer capacity (always power of 2).
    capacity: usize,
    /// Mask for fast modulo: `index & mask` instead of `index % capacity`.
    mask: usize,
    /// Write position (producer increments).
    head: AtomicUsize,
    /// Read position (consumer increments).
    tail: AtomicUsize,
    /// Overflow counter (bytes dropped due to full buffer).
    overflows: AtomicU64,
    /// Underrun counter (times consumer found buffer empty).
    underruns: AtomicU64,
    /// Total bytes written.
    bytes_written: AtomicU64,
    /// Total bytes read.
    bytes_read: AtomicU64,
}

impl AudioRingBuffer {
    /// Creates a new ring buffer with the specified capacity.
    ///
    /// The capacity is rounded up to the next power of 2 for efficient
    /// modulo operations using bit masking.
    ///
    /// # Arguments
    ///
    /// * `capacity_bytes` - Desired capacity in bytes (will be rounded up).
    ///
    /// # Example
    ///
    /// ```
    /// use yard_audio::AudioRingBuffer;
    ///
    /// // Request 50KB, gets rounded to 64KB (next power of 2)
    /// let buffer = AudioRingBuffer::new(50000);
    /// assert_eq!(buffer.capacity(), 65536);
    /// ```
    #[must_use]
    pub fn new(capacity_bytes: usize) -> Self {
        // Ensure minimum capacity and round to power of 2
        let capacity = capacity_bytes.max(1024).next_power_of_two();
        let mask = capacity - 1;

        Self {
            buffer: vec![0u8; capacity].into_boxed_slice(),
            capacity,
            mask,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            overflows: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            bytes_read: AtomicU64::new(0),
        }
    }

    /// Creates a new ring buffer with the default capacity (64KB).
    #[must_use]
    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_BUFFER_CAPACITY)
    }

    /// Returns the buffer capacity in bytes.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the amount of data available to read.
    #[must_use]
    pub fn available_read(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail)
    }

    /// Returns the amount of space available for writing.
    #[must_use]
    pub fn available_write(&self) -> usize {
        self.capacity - self.available_read()
    }

    /// Returns true if the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.available_read() == 0
    }

    /// Returns true if the buffer is full.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.available_read() >= self.capacity
    }

    /// Pushes data into the buffer.
    ///
    /// If the buffer is full, oldest data is dropped to make room (overflow).
    /// Returns the number of bytes actually written.
    ///
    /// # Arguments
    ///
    /// * `data` - The audio data to write.
    ///
    /// # Returns
    ///
    /// The number of bytes written (may be less than `data.len()` if capped).
    pub fn push(&self, data: &[u8]) -> usize {
        if data.is_empty() {
            return 0;
        }

        let len = data.len().min(self.capacity);
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

        let available = self.capacity - head.wrapping_sub(tail);

        // Check for overflow - if we need to drop data
        if len > available {
            let overflow_bytes = len - available;
            self.overflows.fetch_add(1, Ordering::Relaxed);
            // Advance tail to make room (drop oldest samples)
            self.tail.fetch_add(overflow_bytes, Ordering::Release);
        }

        // Write data in up to two segments (handle wraparound)
        let start = head & self.mask;
        let end = (head + len) & self.mask;

        // SAFETY: We're using interior mutability through raw pointers.
        // This is safe because:
        // 1. We're the only writer (SPSC pattern)
        // 2. We update head atomically after writing
        // 3. Consumer only reads data before tail (which we control)
        let buffer_ptr = self.buffer.as_ptr() as *mut u8;

        if start < end || len <= self.capacity - start {
            // Contiguous write (no wraparound)
            // SAFETY: start + len <= capacity, data.len() >= len
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), buffer_ptr.add(start), len);
            }
        } else {
            // Wraparound write - split into two copies
            let first_part = self.capacity - start;
            let second_part = len - first_part;

            // SAFETY: first_part bytes from start to end of buffer
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), buffer_ptr.add(start), first_part);
            }
            // SAFETY: second_part bytes from beginning of buffer
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr().add(first_part),
                    buffer_ptr,
                    second_part,
                );
            }
        }

        // Update head position
        self.head.fetch_add(len, Ordering::Release);
        self.bytes_written.fetch_add(len as u64, Ordering::Relaxed);

        len
    }

    /// Pops data from the buffer into the provided slice.
    ///
    /// If the buffer has less data than requested, the remaining bytes
    /// are filled with silence (zeros) to prevent audio crackling.
    ///
    /// # Arguments
    ///
    /// * `buf` - The buffer to read into.
    ///
    /// # Returns
    ///
    /// The number of bytes actually read from the ring buffer
    /// (not including silence padding).
    pub fn pop(&self, buf: &mut [u8]) -> usize {
        if buf.is_empty() {
            return 0;
        }

        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Relaxed);

        let available = head.wrapping_sub(tail);
        let to_read = buf.len().min(available);

        // Check for underrun
        if to_read < buf.len() {
            self.underruns.fetch_add(1, Ordering::Relaxed);
            // Fill remainder with silence
            buf[to_read..].fill(0);
        }

        if to_read > 0 {
            // Read data in up to two segments (handle wraparound)
            let start = tail & self.mask;

            // SAFETY: We're reading from positions before head, which producer won't touch
            let buffer_ptr = self.buffer.as_ptr();

            if start + to_read <= self.capacity {
                // Contiguous read (no wraparound)
                // SAFETY: start + to_read <= capacity
                unsafe {
                    std::ptr::copy_nonoverlapping(buffer_ptr.add(start), buf.as_mut_ptr(), to_read);
                }
            } else {
                // Wraparound read - split into two copies
                let first_part = self.capacity - start;
                let second_part = to_read - first_part;

                // SAFETY: first_part bytes from start to end of buffer
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        buffer_ptr.add(start),
                        buf.as_mut_ptr(),
                        first_part,
                    );
                }
                // SAFETY: second_part bytes from beginning of buffer
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        buffer_ptr,
                        buf.as_mut_ptr().add(first_part),
                        second_part,
                    );
                }
            }

            // Update tail position
            self.tail.fetch_add(to_read, Ordering::Release);
            self.bytes_read.fetch_add(to_read as u64, Ordering::Relaxed);
        }

        to_read
    }

    /// Clears all data from the buffer.
    ///
    /// This resets head and tail to the same position, effectively
    /// discarding all buffered data. Useful when audio format changes.
    pub fn clear(&self) {
        let head = self.head.load(Ordering::Relaxed);
        self.tail.store(head, Ordering::Release);
    }

    /// Returns current buffer statistics.
    #[must_use]
    pub fn stats(&self) -> RingBufferStats {
        RingBufferStats {
            overflows: self.overflows.load(Ordering::Relaxed),
            underruns: self.underruns.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            current_fill: self.available_read(),
            capacity: self.capacity,
        }
    }

    /// Resets statistics counters to zero.
    pub fn reset_stats(&self) {
        self.overflows.store(0, Ordering::Relaxed);
        self.underruns.store(0, Ordering::Relaxed);
        self.bytes_written.store(0, Ordering::Relaxed);
        self.bytes_read.store(0, Ordering::Relaxed);
    }
}

// SAFETY: AudioRingBuffer uses atomics for all shared state,
// making it safe to share between threads.
//
// Thread safety analysis:
// - head/tail pointers use atomic operations with appropriate Ordering
// - Producer only writes to positions >= tail (controlled by tail atomic)
// - Consumer only reads from positions < head (controlled by head atomic)
// - Buffer data races are prevented by the SPSC invariant: single producer, single consumer
// - Statistics counters are non-critical and use Relaxed ordering
//
// The concurrent test (test_concurrent_access) validates no panics under contention.
// For production validation, run with: MIRIFLAGS="-Zmiri-disable-isolation" cargo +nightly miri test
unsafe impl Sync for AudioRingBuffer {}
unsafe impl Send for AudioRingBuffer {}

impl Default for AudioRingBuffer {
    fn default() -> Self {
        Self::with_default_capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_rounds_to_power_of_two() {
        let buf = AudioRingBuffer::new(50000);
        assert_eq!(buf.capacity(), 65536); // 2^16

        let buf = AudioRingBuffer::new(1000);
        assert_eq!(buf.capacity(), 1024); // 2^10 (minimum)

        let buf = AudioRingBuffer::new(65536);
        assert_eq!(buf.capacity(), 65536); // Already power of 2
    }

    #[test]
    fn test_default_capacity() {
        let buf = AudioRingBuffer::with_default_capacity();
        assert_eq!(buf.capacity(), DEFAULT_BUFFER_CAPACITY);
    }

    #[test]
    fn test_empty_buffer() {
        let buf = AudioRingBuffer::new(1024);
        assert!(buf.is_empty());
        assert!(!buf.is_full());
        assert_eq!(buf.available_read(), 0);
        assert_eq!(buf.available_write(), 1024);
    }

    #[test]
    fn test_basic_push_pop() {
        let buf = AudioRingBuffer::new(1024);

        // Push some data
        let data = [1u8, 2, 3, 4, 5];
        let written = buf.push(&data);
        assert_eq!(written, 5);
        assert_eq!(buf.available_read(), 5);

        // Pop data
        let mut output = [0u8; 5];
        let read = buf.pop(&mut output);
        assert_eq!(read, 5);
        assert_eq!(output, [1, 2, 3, 4, 5]);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_wraparound() {
        let buf = AudioRingBuffer::new(1024);

        // Fill most of the buffer
        let data = vec![0xAA; 900];
        buf.push(&data);

        // Pop most of it
        let mut output = [0u8; 800];
        buf.pop(&mut output);

        // Now push data that wraps around
        let wrap_data = vec![0xBB; 500];
        let written = buf.push(&wrap_data);
        assert_eq!(written, 500);

        // Verify we can read it back correctly
        let mut result = [0u8; 600];
        let read = buf.pop(&mut result);
        assert_eq!(read, 600); // 100 remaining + 500 new

        // First 100 bytes should be 0xAA (leftover)
        assert!(result[..100].iter().all(|&b| b == 0xAA));
        // Next 500 bytes should be 0xBB (new data)
        assert!(result[100..600].iter().all(|&b| b == 0xBB));
    }

    #[test]
    fn test_overflow_drops_oldest() {
        let buf = AudioRingBuffer::new(1024);

        // Fill the buffer completely
        let data = vec![0xAA; 1024];
        buf.push(&data);
        assert!(buf.is_full());

        // Push more data - should overflow
        let new_data = vec![0xBB; 100];
        buf.push(&new_data);

        // Check overflow was recorded
        let stats = buf.stats();
        assert_eq!(stats.overflows, 1);

        // Buffer should still be full with newest data
        assert!(buf.is_full());

        // Read all data - oldest should have been dropped
        let mut output = vec![0u8; 1024];
        buf.pop(&mut output);

        // Last 100 bytes should be 0xBB (new data)
        assert!(output[924..].iter().all(|&b| b == 0xBB));
    }

    #[test]
    fn test_underrun_fills_silence() {
        let buf = AudioRingBuffer::new(1024);

        // Push only 50 bytes
        let data = vec![0xFF; 50];
        buf.push(&data);

        // Try to read 100 bytes
        let mut output = vec![0xAA; 100]; // Pre-fill with non-zero
        let read = buf.pop(&mut output);

        assert_eq!(read, 50); // Only 50 bytes actually read
        assert!(output[..50].iter().all(|&b| b == 0xFF)); // Data
        assert!(output[50..].iter().all(|&b| b == 0)); // Silence

        // Check underrun was recorded
        let stats = buf.stats();
        assert_eq!(stats.underruns, 1);
    }

    #[test]
    fn test_clear() {
        let buf = AudioRingBuffer::new(1024);

        // Add some data
        let data = vec![1u8; 500];
        buf.push(&data);
        assert!(!buf.is_empty());

        // Clear
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.available_read(), 0);
    }

    #[test]
    fn test_stats() {
        let buf = AudioRingBuffer::new(1024);

        let data = vec![1u8; 100];
        buf.push(&data);

        let mut output = [0u8; 50];
        buf.pop(&mut output);

        let stats = buf.stats();
        assert_eq!(stats.bytes_written, 100);
        assert_eq!(stats.bytes_read, 50);
        assert_eq!(stats.current_fill, 50);
        assert_eq!(stats.capacity, 1024);
    }

    #[test]
    fn test_stats_fill_percentage() {
        let stats = RingBufferStats {
            current_fill: 512,
            capacity: 1024,
            ..Default::default()
        };
        assert!((stats.fill_percentage() - 0.5).abs() < f32::EPSILON);

        let empty = RingBufferStats::default();
        assert_eq!(empty.fill_percentage(), 0.0);
    }

    #[test]
    fn test_reset_stats() {
        let buf = AudioRingBuffer::new(1024);

        // Generate some stats
        let data = vec![1u8; 100];
        buf.push(&data);
        let mut output = [0u8; 200]; // Will cause underrun
        buf.pop(&mut output);

        let stats = buf.stats();
        assert!(stats.bytes_written > 0);
        assert!(stats.underruns > 0);

        // Reset
        buf.reset_stats();
        let stats = buf.stats();
        assert_eq!(stats.bytes_written, 0);
        assert_eq!(stats.bytes_read, 0);
        assert_eq!(stats.overflows, 0);
        assert_eq!(stats.underruns, 0);
    }

    #[test]
    fn test_empty_push_pop() {
        let buf = AudioRingBuffer::new(1024);

        // Empty push should return 0
        assert_eq!(buf.push(&[]), 0);

        // Empty pop should return 0
        let mut empty: [u8; 0] = [];
        assert_eq!(buf.pop(&mut empty), 0);
    }

    #[test]
    fn test_data_larger_than_capacity() {
        let buf = AudioRingBuffer::new(1024);

        // Try to push data larger than buffer capacity
        let large_data = vec![0xCC; 2000];
        let written = buf.push(&large_data);

        // Should only write up to capacity
        assert_eq!(written, 1024);

        // Buffer should be full
        assert!(buf.is_full());

        // Verify we can read the data (last 1024 bytes of input)
        let mut output = vec![0u8; 1024];
        let read = buf.pop(&mut output);
        assert_eq!(read, 1024);
        assert!(output.iter().all(|&b| b == 0xCC));
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::thread;

        let buf = Arc::new(AudioRingBuffer::new(65536));
        let buf_producer = Arc::clone(&buf);
        let buf_consumer = Arc::clone(&buf);

        // Signal that producer has started writing
        let producer_started = Arc::new(AtomicBool::new(false));
        let producer_started_clone = Arc::clone(&producer_started);

        // Producer thread
        let producer = thread::spawn(move || {
            let data = vec![0xAB; 1000];
            for i in 0..100 {
                buf_producer.push(&data);
                if i == 0 {
                    producer_started_clone.store(true, Ordering::Release);
                }
                thread::yield_now();
            }
        });

        // Consumer thread - wait for producer to start before consuming
        let consumer = thread::spawn(move || {
            // Spin until producer has written at least once
            while !producer_started.load(Ordering::Acquire) {
                thread::yield_now();
            }

            let mut output = vec![0u8; 500];
            let mut total_read = 0;
            for _ in 0..200 {
                total_read += buf_consumer.pop(&mut output);
                thread::yield_now();
            }
            total_read
        });

        producer.join().expect("Producer panicked");
        let total_read = consumer.join().expect("Consumer panicked");

        // Should have read a significant amount of data
        assert!(total_read > 0);

        // Stats should be consistent
        let stats = buf.stats();
        assert_eq!(stats.bytes_written, 100 * 1000);
    }
}
