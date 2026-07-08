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

    /// Generate a synthetic sine-wave signal in-code (ADR-0007: fixtures must
    /// be synthetic or public-domain, never real recordings).
    fn sine_wave(freq_hz: f32, sample_rate: u32, num_samples: usize, amplitude: f32) -> Vec<f32> {
        (0..num_samples)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                amplitude * (2.0 * std::f32::consts::PI * freq_hz * t).sin()
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn downmix_resample_averages_stereo_channels_at_matching_rate() {
        // input_rate == TARGET_SAMPLE_RATE, so only the downmix applies.
        let stereo = [1.0, -1.0, 0.5, 0.3];
        let mono = downmix_resample(&stereo, 2, TARGET_SAMPLE_RATE);
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.0).abs() < 1e-6);
        assert!((mono[1] - 0.4).abs() < 1e-6);
    }

    #[test]
    fn downmix_resample_passes_mono_through_unchanged_at_matching_rate() {
        let mono_in = vec![0.1, 0.2, -0.3];
        let out = downmix_resample(&mono_in, 1, TARGET_SAMPLE_RATE);
        assert_eq!(out, mono_in);
    }

    #[test]
    fn downmix_resample_upsamples_8khz_to_16khz_expected_length() {
        let input = sine_wave(440.0, 8_000, 4_000, 1.0);
        let out = downmix_resample(&input, 1, 8_000);
        assert_eq!(out.len(), 8_000);
    }

    #[test]
    fn downmix_resample_downsamples_44100_to_16khz_expected_length() {
        let input = sine_wave(440.0, 44_100, 44_100, 1.0);
        let out = downmix_resample(&input, 1, 44_100);
        assert_eq!(out.len(), 16_000);
    }

    #[test]
    fn downmix_resample_preserves_sine_amplitude_within_tolerance() {
        let input = sine_wave(440.0, 8_000, 4_000, 1.0);
        let out = downmix_resample(&input, 1, 8_000);
        let expected_rms = 1.0_f32 / std::f32::consts::SQRT_2;
        assert!(
            (rms(&out) - expected_rms).abs() < 0.05,
            "resampled RMS {} too far from expected {}",
            rms(&out),
            expected_rms
        );
    }

    #[test]
    fn rms_level_of_full_scale_square_wave_is_one() {
        let square = [1.0, -1.0, 1.0, -1.0];
        assert!((rms_level(&square) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rms_level_of_empty_window_is_zero() {
        assert_eq!(rms_level(&[]), 0.0);
    }

    #[test]
    fn peak_level_returns_max_absolute_sample() {
        let samples = [0.1, -0.9, 0.5, 0.2];
        assert!((peak_level(&samples) - 0.9).abs() < 1e-6);
    }

    #[test]
    fn peak_level_of_empty_window_is_zero() {
        assert_eq!(peak_level(&[]), 0.0);
    }

    #[test]
    fn wav_export_round_trips_header_and_sample_count() {
        let samples = sine_wave(440.0, TARGET_SAMPLE_RATE, 1_600, 0.8);
        let path = std::env::temp_dir().join(format!(
            "bla_audio_test_wav_round_trip_{}.wav",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        write_wav_16k_mono(&samples, &path).expect("write_wav_16k_mono should succeed");

        let mut reader = hound::WavReader::open(&path).expect("WAV file should be readable");
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, TARGET_SAMPLE_RATE);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);
        assert_eq!(reader.len() as usize, samples.len());

        let read_back: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.expect("sample should decode") as f32 / i16::MAX as f32)
            .collect();
        assert_eq!(read_back.len(), samples.len());
        for (original, decoded) in samples.iter().zip(read_back.iter()) {
            assert!(
                (original - decoded).abs() < 0.001,
                "round-tripped sample {} too far from original {}",
                decoded,
                original
            );
        }

        let _ = std::fs::remove_file(&path);
    }

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
