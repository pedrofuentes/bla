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

/// A ring buffer shared between the capture callback (writer) and the rest
/// of the app (reader) — the only piece of shared mutable state the OS-glue
/// entry point below touches.
pub type SharedRingBuffer = std::sync::Arc<std::sync::Mutex<RingBuffer>>;

/// Downmix an interleaved multi-channel buffer to mono (by averaging the
/// channels of each frame) and linearly resample it from `input_rate` to
/// [`TARGET_SAMPLE_RATE`], the rate `stt` expects.
///
/// `input` is interleaved: `channels` samples per frame
/// (`[ch0, ch1, ..., ch0, ch1, ...]`). The resampler is a simple linear
/// interpolation — adequate for feeding speech into whisper, not a
/// general-purpose DSP resampler.
pub fn downmix_resample(input: &[f32], channels: u16, input_rate: u32) -> Vec<f32> {
    let mono = downmix_to_mono(input, channels);
    resample_linear(&mono, input_rate, TARGET_SAMPLE_RATE)
}

/// Same transform as [`downmix_resample`], but writes into caller-supplied
/// scratch buffers instead of allocating fresh `Vec`s (issue #58): `mono_scratch`
/// and `out` are cleared and refilled each call, so a caller that reuses the
/// same two buffers across many calls (as the real-time audio callback does)
/// only pays allocation cost the first few times each buffer grows to its
/// steady-state capacity, never per-callback afterward.
pub fn downmix_resample_into(
    input: &[f32],
    channels: u16,
    input_rate: u32,
    mono_scratch: &mut Vec<f32>,
    out: &mut Vec<f32>,
) {
    downmix_to_mono_into(input, channels, mono_scratch);
    resample_linear_into(mono_scratch, input_rate, TARGET_SAMPLE_RATE, out);
}

fn downmix_to_mono(input: &[f32], channels: u16) -> Vec<f32> {
    let mut out = Vec::new();
    downmix_to_mono_into(input, channels, &mut out);
    out
}

fn downmix_to_mono_into(input: &[f32], channels: u16, out: &mut Vec<f32>) {
    out.clear();
    let channels = channels.max(1) as usize;
    if channels == 1 {
        out.extend_from_slice(input);
        return;
    }
    out.extend(
        input
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32),
    );
}

/// Linear-interpolation resampler: maps each output sample index back to a
/// fractional position in the input and interpolates between its two
/// neighboring samples. Documented tradeoff: simple and dependency-free, at
/// the cost of some high-frequency aliasing versus a windowed-sinc resampler
/// — acceptable for speech destined for whisper (ADR-0002).
fn resample_linear(mono: &[f32], input_rate: u32, output_rate: u32) -> Vec<f32> {
    let mut out = Vec::new();
    resample_linear_into(mono, input_rate, output_rate, &mut out);
    out
}

fn resample_linear_into(mono: &[f32], input_rate: u32, output_rate: u32, out: &mut Vec<f32>) {
    out.clear();
    if mono.is_empty() || input_rate == 0 {
        return;
    }
    if input_rate == output_rate {
        out.extend_from_slice(mono);
        return;
    }
    let ratio = output_rate as f64 / input_rate as f64;
    let out_len = ((mono.len() as f64) * ratio).round() as usize;
    let last_idx = mono.len() - 1;
    out.extend((0..out_len).map(|i| {
        let src_pos = i as f64 / ratio;
        let idx = (src_pos.floor() as usize).min(last_idx);
        let frac = (src_pos - idx as f64) as f32;
        let a = mono[idx];
        let b = mono[(idx + 1).min(last_idx)];
        a + (b - a) * frac
    }));
}

/// Root-mean-square level of a sample window — used to drive the pill's live
/// waveform meter. Returns `0.0` for an empty window.
pub fn rms_level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Peak (max absolute value) level of a sample window. Returns `0.0` for an
/// empty window.
pub fn peak_level(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0_f32, |acc, &s| acc.max(s.abs()))
}

/// Write a captured window of 16 kHz mono `f32` samples out as a 16-bit PCM
/// WAV file, so the pipeline and tests can round-trip a captured window
/// (e.g. as an `stt` input fixture).
pub fn write_wav_16k_mono(samples: &[f32], path: &std::path::Path) -> std::io::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(hound_to_io_err)?;
    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let pcm = (clamped * i16::MAX as f32).round() as i16;
        writer.write_sample(pcm).map_err(hound_to_io_err)?;
    }
    writer.finalize().map_err(hound_to_io_err)
}

fn hound_to_io_err(err: hound::Error) -> std::io::Error {
    std::io::Error::other(err)
}

/// OS-integration glue (AGENTS.md §OS-integration exemption): opens the
/// default input device and streams captured audio into `buffer`. Every
/// decision — downmixing, resampling, overflow behavior, contention/error
/// bookkeeping — is delegated to the pure functions/types above; this
/// function only wires cpal callbacks to them, so it stays thin and
/// untested (no audio device in CI).
///
/// Issue #58: the callback captures two scratch buffers instead of
/// allocating fresh `Vec`s per call, and uses [`Mutex::try_lock`] instead of
/// a blocking `lock()` — a contended lock drops that callback's samples and
/// counts the drop via `diagnostics` rather than stalling the real-time
/// audio thread. Issue #59: a poisoned lock or a stream error is recorded
/// into `diagnostics` (structured state the rest of the app can observe)
/// instead of an invisible `eprintln!`.
pub fn start_capture(
    buffer: SharedRingBuffer,
    diagnostics: std::sync::Arc<CaptureDiagnostics>,
) -> Result<cpal::Stream, CaptureError> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or(CaptureError::NoInputDevice)?;
    let config = device.default_input_config()?;
    let channels = config.channels();
    let input_rate = config.sample_rate();

    let mut mono_scratch: Vec<f32> = Vec::new();
    let mut resampled_scratch: Vec<f32> = Vec::new();
    let callback_diagnostics = diagnostics.clone();
    let error_diagnostics = diagnostics;

    let stream = device.build_input_stream(
        config.into(),
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            downmix_resample_into(
                data,
                channels,
                input_rate,
                &mut mono_scratch,
                &mut resampled_scratch,
            );
            match buffer.try_lock() {
                Ok(mut buf) => buf.extend(&resampled_scratch),
                Err(std::sync::TryLockError::WouldBlock) => {
                    callback_diagnostics.record_dropped_callback();
                }
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    callback_diagnostics.record_error(CaptureRuntimeError::BufferLockPoisoned);
                }
            }
        },
        move |err| error_diagnostics.record_error(CaptureRuntimeError::Stream(err.to_string())),
        None,
    )?;
    stream.play()?;
    Ok(stream)
}

/// Runs [`start_capture`] on a dedicated thread for the lifetime of one
/// hold-to-record session (OS-integration glue, thin): `cpal::Stream` is not
/// guaranteed `Send` on every platform backend, so rather than move it
/// across threads, the thread that creates it also owns it until
/// [`CaptureSession::stop`] signals it to drop the stream and exit. This is
/// the type `hotkeys`' `StartRecording`/`StopRecording` transitions (wired in
/// `lib.rs`) actually start/stop.
pub struct CaptureSession {
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

/// Maximum time [`CaptureSession::start`] waits for the audio backend to
/// acquire the device and confirm the stream is running before giving up
/// (Sentinel 🟡 #4). Bounded so a hung backend (an exclusively-locked
/// device, a Bluetooth-mic renegotiation that stalls) can't block the
/// hotkey-callback thread indefinitely — generous enough not to trip on a
/// merely-slow-but-live device.
pub const CAPTURE_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

impl CaptureSession {
    /// Starts capturing into `buffer` on a dedicated thread, blocking until
    /// the stream is confirmed running (or has failed to start), but no
    /// longer than [`CAPTURE_START_TIMEOUT`].
    pub fn start(
        buffer: SharedRingBuffer,
        diagnostics: std::sync::Arc<CaptureDiagnostics>,
    ) -> Result<Self, CaptureError> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), CaptureError>>();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();

        let handle = std::thread::spawn(move || match start_capture(buffer, diagnostics) {
            Ok(stream) => {
                if ready_tx.send(Ok(())).is_err() {
                    return;
                }
                // Block here, keeping `stream` alive, until told to stop.
                let _ = stop_rx.recv();
                drop(stream);
            }
            Err(err) => {
                let _ = ready_tx.send(Err(err));
            }
        });

        // Sentinel 🟡 #4: bound the wait so a hung backend surfaces as
        // CaptureError::Timeout (which the caller's existing Err handling
        // turns into PipelineState::Error) instead of blocking the
        // hotkey-callback thread forever. On timeout we deliberately do NOT
        // join the spawned thread — it may still be stuck inside the
        // backend's device setup — and instead detach it (dropping its
        // JoinHandle); it will exit on its own once setup returns and it
        // finds the ready channel closed.
        match ready_rx.recv_timeout(CAPTURE_START_TIMEOUT) {
            Ok(Ok(())) => Ok(Self {
                stop_tx: Some(stop_tx),
                handle: Some(handle),
            }),
            Ok(Err(err)) => {
                let _ = handle.join();
                Err(err)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(CaptureError::Timeout),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = handle.join();
                Err(CaptureError::NoInputDevice)
            }
        }
    }

    /// Signals the capture thread to drop the stream and stops capturing.
    /// Blocks until the thread has exited.
    pub fn stop(mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CaptureSession {
    /// A dropped-without-`stop()` session (e.g. an early return in the
    /// caller) still signals the capture thread to exit rather than leaking
    /// it — `stop()` remains the normal path since it also joins.
    fn drop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Errors from opening the input device or building/starting the capture
/// stream — OS-glue error plumbing, not decision logic.
#[derive(Debug)]
pub enum CaptureError {
    NoInputDevice,
    Cpal(cpal::Error),
    /// The audio backend did not confirm the stream was running within
    /// [`CAPTURE_START_TIMEOUT`] (Sentinel 🟡 #4) — a hung device
    /// acquisition, surfaced instead of blocking the caller forever.
    Timeout,
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::NoInputDevice => write!(f, "no default input audio device available"),
            CaptureError::Cpal(e) => write!(f, "audio capture error: {e}"),
            CaptureError::Timeout => {
                write!(
                    f,
                    "timed out waiting for the audio device to start capturing"
                )
            }
        }
    }
}

impl std::error::Error for CaptureError {}

impl From<cpal::Error> for CaptureError {
    fn from(e: cpal::Error) -> Self {
        CaptureError::Cpal(e)
    }
}

/// A structured problem observed on the real-time capture thread — the
/// discriminated replacement for an invisible `eprintln!` (issues #44/#59):
/// a poisoned ring-buffer lock, or a `cpal` stream error reported by its
/// error callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureRuntimeError {
    /// [`SharedRingBuffer`]'s mutex was poisoned (a prior panic while the
    /// lock was held) — captured audio can no longer be trusted.
    BufferLockPoisoned,
    /// The underlying `cpal` stream reported an error via its error
    /// callback (device disconnected, format renegotiation failure, ...).
    Stream(String),
}

impl std::fmt::Display for CaptureRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureRuntimeError::BufferLockPoisoned => {
                write!(f, "audio ring buffer lock was poisoned")
            }
            CaptureRuntimeError::Stream(msg) => write!(f, "audio capture stream error: {msg}"),
        }
    }
}

/// Shared, thread-safe capture diagnostics the real-time callback (writer)
/// reports into and the rest of the app (reader) observes (issues #44/#59,
/// #58). Pure bookkeeping over an atomic counter and a small mutex-guarded
/// slot — no OS calls, fully unit-testable without a real audio device.
///
/// [`Self::record_dropped_callback`] is called instead of blocking when the
/// ring-buffer lock is contended (issue #58: the real-time thread must never
/// block on a contended `std::sync::Mutex`, so a contended callback drops its
/// samples and counts the drop rather than waiting); [`Self::record_error`]
/// is called instead of `eprintln!` for a poisoned lock or a `cpal` stream
/// error (issue #59), so the app can surface degraded-capture state (e.g. an
/// error tray icon) instead of it vanishing into a packaged app's invisible
/// stderr.
#[derive(Debug, Default)]
pub struct CaptureDiagnostics {
    dropped_callbacks: std::sync::atomic::AtomicU64,
    last_error: std::sync::Mutex<Option<CaptureRuntimeError>>,
}

impl CaptureDiagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that one real-time callback dropped its samples because the
    /// ring-buffer lock was contended (issue #58).
    pub fn record_dropped_callback(&self) {
        self.dropped_callbacks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Total callbacks that have dropped samples due to lock contention so
    /// far.
    pub fn dropped_callbacks(&self) -> u64 {
        self.dropped_callbacks
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Record a structured capture error (issue #59), overwriting whatever
    /// was previously recorded. Best-effort: if the diagnostics mutex itself
    /// were ever poisoned, this silently no-ops rather than panicking the
    /// real-time thread.
    pub fn record_error(&self, error: CaptureRuntimeError) {
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = Some(error);
        }
    }

    /// The most recently recorded capture error, if any.
    pub fn last_error(&self) -> Option<CaptureRuntimeError> {
        self.last_error.lock().ok().and_then(|guard| guard.clone())
    }

    /// Clear the recorded error (e.g. once the app has surfaced/acknowledged
    /// it).
    pub fn clear_error(&self) {
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = None;
        }
    }
}

/// Throttles `audio-level` event emission (issue #126, M2 PR 2.2) so the
/// pill's live meter isn't flooded with one event per captured audio chunk
/// -- a cpal callback commonly fires far faster than any UI needs to
/// repaint. Pure and deterministic (no clock): `now` is an injected,
/// caller-supplied monotonic timestamp -- e.g. `Instant::now().elapsed()`
/// in the real glue -- mirroring `hotkeys::Timestamp` (see that module's
/// doc), so this is fully unit-testable without a real audio device or
/// wall-clock timing.
#[derive(Debug)]
pub struct LevelThrottle {
    min_interval: std::time::Duration,
    last_emitted_at: Option<std::time::Duration>,
}

impl LevelThrottle {
    /// ~30 Hz cadence: emits at most once per this interval.
    pub const DEFAULT_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

    /// A throttle using [`Self::DEFAULT_MIN_INTERVAL`] (~30 Hz).
    pub fn new() -> Self {
        Self::with_min_interval(Self::DEFAULT_MIN_INTERVAL)
    }

    /// A throttle using a caller-chosen minimum interval between emits
    /// (tests use this to exercise the cadence logic without depending on
    /// the production constant).
    pub fn with_min_interval(min_interval: std::time::Duration) -> Self {
        Self {
            min_interval,
            last_emitted_at: None,
        }
    }

    /// Whether `rms_level` observed at `now` should be emitted now, given
    /// everything observed so far. The very first observation always
    /// emits (issue #126: the pill's meter should reflect the first
    /// sample immediately rather than waiting out a full throttle window);
    /// after that, at most one emit per [`Self::min_interval`] elapsed
    /// since the previous emit. Gating is purely time-based -- the level
    /// value itself never fast-tracks or delays an emit, and a suppressed
    /// sample is dropped outright rather than averaged/smoothed into the
    /// next emitted value.
    pub fn should_emit(&mut self, now: std::time::Duration, rms_level: f32) -> Option<f32> {
        let should_emit = match self.last_emitted_at {
            Some(last) => now.saturating_sub(last) >= self.min_interval,
            None => true,
        };
        if should_emit {
            self.last_emitted_at = Some(now);
            Some(rms_level)
        } else {
            None
        }
    }
}

impl Default for LevelThrottle {
    fn default() -> Self {
        Self::new()
    }
}

/// RT-safe latest-level cell (issue #126, M2 PR 2.2): the capture callback
/// (writer, on the real-time `cpal` thread) records the RMS level of each
/// captured chunk here via a lock-free atomic store -- unlike
/// [`SharedRingBuffer`]'s `Mutex` (which the callback only ever
/// `try_lock`s), this can never block, even briefly. The `audio-level`
/// event poller (reader, thin glue in `lib.rs`) samples it well below the
/// callback's own rate and pushes each sample through [`LevelThrottle`]
/// before emitting -- this cell only ever carries a scalar, never raw
/// samples (MISSION.md §7: audio samples never leave `audio.rs` as
/// events).
///
/// `f32` has no native stable atomic type, so the level is bit-cast into
/// an `AtomicU32` ([`f32::to_bits`]/[`f32::from_bits`]) -- a standard,
/// allocation-free encoding; `Relaxed` ordering is sufficient since this
/// cell carries one independent scalar with no other memory it needs to
/// stay synchronized with.
#[derive(Debug, Default)]
pub struct LevelMeter {
    bits: std::sync::atomic::AtomicU32,
}

impl LevelMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the latest observed RMS level. Called from the real-time
    /// capture callback -- must stay lock-free/allocation-free.
    pub fn record(&self, level: f32) {
        self.bits
            .store(level.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }

    /// The most recently recorded level, or `0.0` if none has been
    /// recorded yet (matching [`rms_level`]'s empty-window default).
    pub fn current(&self) -> f32 {
        f32::from_bits(self.bits.load(std::sync::atomic::Ordering::Relaxed))
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

    // -----------------------------------------------------------------
    // Issue #58: scratch-buffer (allocation-free) downmix/resample variant
    // -----------------------------------------------------------------

    #[test]
    fn downmix_resample_into_matches_the_allocating_version() {
        let stereo = [1.0, -1.0, 0.5, 0.3, 0.2, -0.2];
        let expected = downmix_resample(&stereo, 2, TARGET_SAMPLE_RATE);

        let mut mono_scratch = Vec::new();
        let mut out = Vec::new();
        downmix_resample_into(&stereo, 2, TARGET_SAMPLE_RATE, &mut mono_scratch, &mut out);

        assert_eq!(out, expected);
    }

    #[test]
    fn downmix_resample_into_reuses_buffers_without_leaking_prior_call_data() {
        let mut mono_scratch = Vec::new();
        let mut out = Vec::new();

        // First call: a long buffer.
        let first = sine_wave(440.0, TARGET_SAMPLE_RATE, 1_000, 1.0);
        downmix_resample_into(&first, 1, TARGET_SAMPLE_RATE, &mut mono_scratch, &mut out);
        assert_eq!(out.len(), 1_000);
        let capacity_after_first = out.capacity();

        // Second call: a much shorter buffer — `out` must reflect ONLY the
        // second call's contents (proving `clear()` ran), while its
        // capacity is retained from the first call rather than being
        // reallocated from scratch (the entire point of the scratch-buffer
        // seam: steady-state calls shouldn't (re)allocate).
        let second = vec![0.25_f32; 10];
        downmix_resample_into(&second, 1, TARGET_SAMPLE_RATE, &mut mono_scratch, &mut out);
        assert_eq!(out.len(), 10);
        assert_eq!(
            out, second,
            "stale samples from the first call must not linger"
        );
        assert!(
            out.capacity() >= capacity_after_first,
            "capacity should be retained/reused across calls, not shrunk"
        );
    }

    #[test]
    fn downmix_resample_into_handles_stereo_downmix_and_resample_together() {
        let input = sine_wave(440.0, 8_000, 2_000, 1.0);
        // Interleave into a fake stereo signal (duplicate channel).
        let stereo: Vec<f32> = input.iter().flat_map(|&s| [s, s]).collect();

        let mut mono_scratch = Vec::new();
        let mut out = Vec::new();
        downmix_resample_into(&stereo, 2, 8_000, &mut mono_scratch, &mut out);

        let expected = downmix_resample(&stereo, 2, 8_000);
        assert_eq!(out, expected);
        assert_eq!(out.len(), 4_000); // 8kHz -> 16kHz doubles the sample count
    }

    // -----------------------------------------------------------------
    // Issues #44/#59: structured capture diagnostics
    // -----------------------------------------------------------------

    #[test]
    fn capture_diagnostics_starts_with_no_drops_and_no_error() {
        let diag = CaptureDiagnostics::new();
        assert_eq!(diag.dropped_callbacks(), 0);
        assert_eq!(diag.last_error(), None);
    }

    #[test]
    fn capture_diagnostics_counts_dropped_callbacks() {
        let diag = CaptureDiagnostics::new();
        diag.record_dropped_callback();
        diag.record_dropped_callback();
        diag.record_dropped_callback();
        assert_eq!(diag.dropped_callbacks(), 3);
    }

    #[test]
    fn capture_diagnostics_records_and_overwrites_the_last_error() {
        let diag = CaptureDiagnostics::new();
        diag.record_error(CaptureRuntimeError::BufferLockPoisoned);
        assert_eq!(
            diag.last_error(),
            Some(CaptureRuntimeError::BufferLockPoisoned)
        );

        diag.record_error(CaptureRuntimeError::Stream(
            "device disconnected".to_string(),
        ));
        assert_eq!(
            diag.last_error(),
            Some(CaptureRuntimeError::Stream(
                "device disconnected".to_string()
            )),
            "a newer error must overwrite the older one, not accumulate"
        );
    }

    #[test]
    fn capture_diagnostics_clear_error_resets_to_none() {
        let diag = CaptureDiagnostics::new();
        diag.record_error(CaptureRuntimeError::BufferLockPoisoned);
        diag.clear_error();
        assert_eq!(diag.last_error(), None);
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

    // -----------------------------------------------------------------
    // Issue #126 (M2 PR 2.2): LevelThrottle — throttled `audio-level` event
    // cadence. Pure and deterministic: every timestamp below is injected
    // (`Duration`), never a real clock read, matching hotkeys::Timestamp's
    // pattern (module doc).
    // -----------------------------------------------------------------

    #[test]
    fn level_throttle_emits_the_very_first_observation_immediately() {
        let mut throttle = LevelThrottle::with_min_interval(std::time::Duration::from_millis(33));
        assert_eq!(
            throttle.should_emit(std::time::Duration::from_millis(0), 0.42),
            Some(0.42)
        );
    }

    #[test]
    fn level_throttle_suppresses_a_second_observation_within_the_same_window() {
        let mut throttle = LevelThrottle::with_min_interval(std::time::Duration::from_millis(33));
        assert_eq!(
            throttle.should_emit(std::time::Duration::from_millis(0), 0.1),
            Some(0.1)
        );
        assert_eq!(
            throttle.should_emit(std::time::Duration::from_millis(10), 0.2),
            None,
            "an observation inside the same ~33ms window must be suppressed"
        );
    }

    #[test]
    fn level_throttle_emits_again_once_the_window_elapses() {
        let mut throttle = LevelThrottle::with_min_interval(std::time::Duration::from_millis(33));
        assert_eq!(
            throttle.should_emit(std::time::Duration::from_millis(0), 0.1),
            Some(0.1)
        );
        assert_eq!(
            throttle.should_emit(std::time::Duration::from_millis(33), 0.2),
            Some(0.2),
            "a full window (>=33ms) after the last emit must emit again"
        );
    }

    #[test]
    fn level_throttle_caps_emit_rate_to_at_most_30_per_second_under_a_burst() {
        let mut throttle = LevelThrottle::new();
        let mut emitted = 0usize;
        // A burst of 1000 samples spaced 1ms apart -- far faster than any
        // real cpal callback cadence -- covering exactly one second.
        for ms in 0..1000u64 {
            if throttle
                .should_emit(std::time::Duration::from_millis(ms), 0.5)
                .is_some()
            {
                emitted += 1;
            }
        }
        assert!(
            emitted <= 31,
            "should emit at most ~30 times per second, got {emitted}"
        );
        assert!(
            emitted >= 28,
            "should still emit close to 30 times per second, got {emitted}"
        );
    }

    #[test]
    fn level_throttle_quiet_to_loud_transition_is_suppressed_then_emitted_unsmoothed() {
        let mut throttle = LevelThrottle::with_min_interval(std::time::Duration::from_millis(33));
        assert_eq!(
            throttle.should_emit(std::time::Duration::from_millis(0), 0.01),
            Some(0.01)
        );
        // A much louder level lands inside the same window: cadence gating
        // applies regardless of how much the level itself changed.
        assert_eq!(
            throttle.should_emit(std::time::Duration::from_millis(10), 0.95),
            None
        );
        // Once the window elapses, the loud level is emitted exactly as
        // observed -- no averaging/smoothing across the suppressed samples.
        assert_eq!(
            throttle.should_emit(std::time::Duration::from_millis(33), 0.95),
            Some(0.95)
        );
    }

    #[test]
    fn level_throttle_default_uses_the_documented_30hz_cadence() {
        let mut default_throttle = LevelThrottle::new();
        let mut explicit_throttle =
            LevelThrottle::with_min_interval(LevelThrottle::DEFAULT_MIN_INTERVAL);
        for ms in [0u64, 10, 33, 40, 66] {
            assert_eq!(
                default_throttle.should_emit(std::time::Duration::from_millis(ms), 0.3),
                explicit_throttle.should_emit(std::time::Duration::from_millis(ms), 0.3),
            );
        }
    }

    // -----------------------------------------------------------------
    // Issue #126 (M2 PR 2.2): LevelMeter — the RT-safe latest-level cell
    // the capture callback (writer) records into and the level-event
    // poller (reader, in lib.rs) samples. Lock-free (atomic), never a
    // `Mutex`, so it can never block the real-time audio thread.
    // -----------------------------------------------------------------

    #[test]
    fn level_meter_starts_at_zero() {
        let meter = LevelMeter::new();
        assert_eq!(meter.current(), 0.0);
    }

    #[test]
    fn level_meter_reads_back_the_latest_recorded_level() {
        let meter = LevelMeter::new();
        meter.record(0.25);
        assert_eq!(meter.current(), 0.25);
        meter.record(0.75);
        assert_eq!(
            meter.current(),
            0.75,
            "current() must reflect the latest record(), not accumulate"
        );
    }
}
