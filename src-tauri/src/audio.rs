//! Audio capture: `cpal` input stream → 16 kHz mono `f32` ring buffer.
//!
//! Opens the default input device, resamples/downmixes to the format `stt`
//! expects, and buffers samples for the duration of a hold-to-record session.
//!
//! OS-integration module (AGENTS.md §OS-integration exemption): thin glue only —
//! no cleanup/decision logic lives here. All decisions — buffering, overflow
//! behavior, resampling, level metering, WAV export — live in pure functions
//! below, fully unit-tested without needing a real audio device (ADR-0002,
//! ADR-0007: fixtures are synthetic signals generated in-code, never real
//! recordings).

/// Sample rate the STT stage (`stt.rs`) expects all captured audio to be
/// resampled to before transcription.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Fixed-capacity ring buffer of `f32` audio samples.
///
/// Pure logic (no OS calls) — TDD-mandatory. Once at capacity, pushing a new
/// sample drops the oldest buffered sample first, so a hold-to-record session
/// always keeps the most recent window of audio.
#[derive(Debug)]
pub struct RingBuffer {
    capacity: usize,
    buf: std::collections::VecDeque<f32>,
}

impl RingBuffer {
    /// Create an empty ring buffer that holds at most `capacity` samples.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            buf: std::collections::VecDeque::with_capacity(capacity),
        }
    }

    /// Maximum number of samples this buffer can hold.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of samples currently buffered.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// True if no samples are currently buffered.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Push one sample. If the buffer is at capacity, the oldest sample is
    /// dropped first.
    pub fn push(&mut self, sample: f32) {
        if self.capacity == 0 {
            return;
        }
        if self.buf.len() == self.capacity {
            self.buf.pop_front();
        }
        self.buf.push_back(sample);
    }

    /// Push a slice of samples, applying the same overflow behavior as
    /// [`RingBuffer::push`] to each one in order.
    pub fn extend(&mut self, samples: &[f32]) {
        for &sample in samples {
            self.push(sample);
        }
    }

    /// Copy out the currently buffered window, oldest sample first, without
    /// clearing the buffer.
    pub fn window(&self) -> Vec<f32> {
        self.buf.iter().copied().collect()
    }

    /// Remove and return all buffered samples, oldest sample first, leaving
    /// the buffer empty.
    pub fn drain(&mut self) -> Vec<f32> {
        self.buf.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_buffer_is_empty() {
        let rb = RingBuffer::new(4);
        assert_eq!(rb.capacity(), 4);
        assert_eq!(rb.len(), 0);
        assert!(rb.is_empty());
        assert!(rb.window().is_empty());
    }

    #[test]
    fn push_below_capacity_keeps_all_samples_in_order() {
        let mut rb = RingBuffer::new(4);
        rb.push(1.0);
        rb.push(2.0);
        rb.push(3.0);
        assert_eq!(rb.len(), 3);
        assert_eq!(rb.window(), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn push_past_capacity_drops_oldest_sample_first() {
        let mut rb = RingBuffer::new(3);
        rb.extend(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        // Capacity 3, pushed 5 samples: oldest (1.0, 2.0) dropped.
        assert_eq!(rb.len(), 3);
        assert_eq!(rb.window(), vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn extend_applies_overflow_per_sample() {
        let mut rb = RingBuffer::new(2);
        rb.extend(&[1.0, 2.0]);
        rb.extend(&[3.0]);
        assert_eq!(rb.window(), vec![2.0, 3.0]);
    }

    #[test]
    fn drain_empties_buffer_and_returns_window_order() {
        let mut rb = RingBuffer::new(4);
        rb.extend(&[1.0, 2.0, 3.0]);
        let drained = rb.drain();
        assert_eq!(drained, vec![1.0, 2.0, 3.0]);
        assert!(rb.is_empty());
        assert_eq!(rb.len(), 0);
    }
}
