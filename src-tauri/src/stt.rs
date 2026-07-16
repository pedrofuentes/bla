//! Speech-to-text via `whisper-rs` (whisper.cpp bindings), Metal-accelerated on macOS.
//!
//! Transcribes the audio buffer produced by `audio`. Personal-dictionary terms
//! (from `store`) are passed as Whisper's `initial_prompt` to bias recognition
//! toward the user's vocabulary.
//!
//! Pure-logic-adjacent: the whisper.cpp call is native glue, but pre/post
//! processing (prompt construction, output normalization) should stay unit-testable.
//!
//! `WhisperStt` (the real whisper-rs-backed engine) lives behind the
//! default-off `whisper` cargo feature (see `Cargo.toml`), because whisper-rs
//! builds whisper.cpp from source (heavy native build, needs cmake/clang) and
//! CI/dev environments here don't ship a model file to transcribe with
//! anyway. With the feature off, `cargo test` still exercises the real
//! coverage for this module: the `Stt` trait, `FakeStt` (pipeline-shape test
//! double), and `build_initial_prompt` (the pure, unit-tested AC-21
//! dictionary-seam logic). Enable `--features whisper` to compile and use
//! `WhisperStt` for real transcription.

#[cfg(feature = "whisper")]
use std::path::Path;

/// Pure decision for whether opt-in performance logging is active, given the
/// raw value of the `BLA_PERF_LOG` environment variable (`None` = unset).
///
/// Perf logging (issue #115 follow-up) is a measurement aid for the
/// dictation hot path — model-load duration, per-dictation transcription
/// latency, and cache hit/miss — so the caching/decode-tuning win can be
/// read as numbers instead of judged by feel. It's **off by default** and
/// enabled only when the variable is present and not an explicit disable
/// (`"0"` or empty), so a normal run stays silent and `BLA_PERF_LOG=1`
/// turns it on. Factored out as a pure function so the gating rule is
/// unit-tested without touching real process env or stderr.
pub fn perf_logging_enabled(env_value: Option<&str>) -> bool {
    match env_value {
        None => false,
        Some(value) => !value.is_empty() && value != "0",
    }
}

/// Emits `msg` to stderr as a `bla[perf]` line, but only when
/// [`perf_logging_enabled`] says so for the current `BLA_PERF_LOG` value
/// (read once and cached). Callers pass timing/enum diagnostics **only** —
/// never transcript, clipboard, or audio content (MISSION §7 no-log
/// invariant); this helper is for measuring latency, not inspecting output.
pub fn perf_log(msg: &str) {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    let enabled = *ENABLED
        .get_or_init(|| perf_logging_enabled(std::env::var("BLA_PERF_LOG").ok().as_deref()));
    if enabled {
        eprintln!("bla[perf]: {msg}");
    }
}

/// Errors returned by [`Stt::transcribe`] or [`WhisperStt::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttError {
    /// The whisper.cpp model file could not be loaded (missing, corrupt, or
    /// an unsupported format).
    ModelLoad(String),
    /// The transcription pipeline itself failed after the model loaded.
    Transcription(String),
}

impl std::fmt::Display for SttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SttError::ModelLoad(msg) => write!(f, "failed to load whisper model: {msg}"),
            SttError::Transcription(msg) => write!(f, "transcription failed: {msg}"),
        }
    }
}

impl std::error::Error for SttError {}

/// Options controlling a single [`Stt::transcribe`] call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscribeOpts {
    /// Personal-dictionary terms (AC-21 seam, ADR-0004): rendered into
    /// Whisper's `initial_prompt` via [`build_initial_prompt`] to bias
    /// recognition toward the user's vocabulary. Empty in M1 (the
    /// dictionary itself ships in M3); the seam exists now so this API
    /// never changes when M3 starts populating it.
    pub dictionary: Vec<String>,
}

impl TranscribeOpts {
    /// The `initial_prompt` string to pass to whisper for this call.
    pub fn initial_prompt(&self) -> String {
        build_initial_prompt(&self.dictionary)
    }
}

/// Maximum length, in bytes, of a rendered `initial_prompt`.
///
/// whisper.cpp doesn't hard-reject an overlong prompt — it silently uses
/// only the tail of it that fits the model's prompt-token budget. Rather
/// than rely on that silent, model-dependent truncation, `build_initial_prompt`
/// caps the *rendered* prompt deterministically at a whole-term boundary, so
/// callers get predictable, reproducible output regardless of model.
pub const INITIAL_PROMPT_MAX_CHARS: usize = 1024;

/// Renders dictionary terms into the comma-separated string used as
/// Whisper's `initial_prompt` (AC-21 / ADR-0004). Pure and deterministic:
///
/// - NUL bytes inside a term are stripped (issue #69 — Rust `String`s can
///   contain interior NULs, but a NUL surviving to whisper-rs's
///   `set_initial_prompt` makes its internal `CString::new` reject it and
///   panic; a term left empty by stripping is dropped like any other blank
///   term).
/// - Blank/whitespace-only terms are dropped.
/// - Internal whitespace (including newlines/tabs) in each term collapses to
///   single spaces, so the prompt stays one line.
/// - Terms are joined in the order given (callers control precedence) with
///   `", "` as the separator. Callers that want a specific tie-break under
///   the length cap below control it by ordering `terms` accordingly (see
///   `Store::list_terms`'s newest-first policy, issue #70).
/// - A literal `\` or `,` inside a term is escaped (`\\`, `\,`) so the join
///   stays unambiguous.
/// - The result is capped at [`INITIAL_PROMPT_MAX_CHARS`] bytes. A term that
///   doesn't fit is *skipped*, not treated as an end-of-input signal (issue
///   #70) — an earlier oversized term no longer silently drops every term
///   that follows it; whatever combination of terms fits, in the given
///   order, is what's rendered.
pub fn build_initial_prompt(terms: &[String]) -> String {
    let mut rendered = String::new();

    for term in terms {
        let sanitized: String = term.chars().filter(|&c| c != '\0').collect();
        let collapsed = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            continue;
        }
        let escaped = collapsed.replace('\\', "\\\\").replace(',', "\\,");

        let separator_len = if rendered.is_empty() { 0 } else { 2 };
        if rendered.len() + separator_len + escaped.len() > INITIAL_PROMPT_MAX_CHARS {
            continue;
        }

        if !rendered.is_empty() {
            rendered.push_str(", ");
        }
        rendered.push_str(&escaped);
    }

    rendered
}

/// A speech-to-text engine: raw 16 kHz mono `f32` samples in (as produced by
/// `audio`), recognized text out.
pub trait Stt {
    fn transcribe(&self, samples: &[f32], opts: &TranscribeOpts) -> Result<String, SttError>;
}

/// Test double returning a canned transcript regardless of input — for
/// pipeline-shape tests that need an `Stt` without a real model.
pub struct FakeStt {
    canned: String,
}

impl FakeStt {
    pub fn new(canned: impl Into<String>) -> Self {
        Self {
            canned: canned.into(),
        }
    }
}

impl Default for FakeStt {
    fn default() -> Self {
        Self::new("the quick brown fox jumps over the lazy dog")
    }
}

impl Stt for FakeStt {
    fn transcribe(&self, _samples: &[f32], _opts: &TranscribeOpts) -> Result<String, SttError> {
        Ok(self.canned.clone())
    }
}

/// `Stt` backed by `whisper-rs` (whisper.cpp), Metal-accelerated on macOS
/// (ADR-0004). Behind the default-off `whisper` cargo feature — see the
/// module doc comment.
#[cfg(feature = "whisper")]
pub struct WhisperStt {
    context: whisper_rs::WhisperContext,
}

#[cfg(feature = "whisper")]
impl WhisperStt {
    /// Loads a whisper.cpp model from `model_path`. The path is resolved by
    /// the caller from settings / the OS app-data model dir (ADR-0004); this
    /// module has no knowledge of config/settings itself.
    ///
    /// Loading the model is native glue (TDD-exempt) — not unit-tested here;
    /// see the `#[ignore]`d integration test below for a manual, model-file
    /// smoke test.
    pub fn new(model_path: impl AsRef<Path>) -> Result<Self, SttError> {
        let model_path = model_path.as_ref();
        // Issue #115 decode tuning: flash attention is whisper.cpp's fused
        // attention-computation kernel — a pure decode-latency win with no
        // accuracy cost for the greedy, non-DTW decoding `transcribe` below
        // does (the one thing flash attention can't combine with is DTW
        // token-level timestamps, which this engine never requests).
        let mut params = whisper_rs::WhisperContextParameters::default();
        params.flash_attn(true);
        // Perf instrumentation (issue #115 follow-up): time the ~574 MB
        // context load so it can be confirmed to happen ONCE (at warm/first
        // build) rather than per dictation. Off unless BLA_PERF_LOG is set.
        let load_start = std::time::Instant::now();
        let context = whisper_rs::WhisperContext::new_with_params(model_path, params)
            .map_err(|e| SttError::ModelLoad(format!("{e} (path: {})", model_path.display())))?;
        perf_log(&format!(
            "whisper model context loaded in {} ms",
            load_start.elapsed().as_millis()
        ));
        Ok(Self { context })
    }
}

/// Lets a cached, shared [`WhisperStt`] (issue #115: `AppState::stt_cache`
/// hands dictation threads an `Arc` clone of the already-loaded engine
/// instead of rebuilding it) satisfy [`Pipeline`](crate::pipeline::Pipeline)'s
/// `S: Stt` bound directly, with no wrapper type. Delegates via
/// `Arc::as_ref` rather than calling `self.transcribe(..)` (which would
/// recurse on this very impl); the underlying `WhisperContext` is
/// `Send + Sync`, so sharing it behind an `Arc` across dictation threads is
/// sound, and `transcribe` still creates a fresh `WhisperState` per call
/// either way.
#[cfg(feature = "whisper")]
impl Stt for std::sync::Arc<WhisperStt> {
    fn transcribe(&self, samples: &[f32], opts: &TranscribeOpts) -> Result<String, SttError> {
        self.as_ref().transcribe(samples, opts)
    }
}

#[cfg(feature = "whisper")]
impl Stt for WhisperStt {
    /// Runs whisper.cpp's full transcription pipeline. Native glue
    /// (TDD-exempt): the pure logic it depends on — `build_initial_prompt`
    /// — is unit-tested above; this method itself is only covered by the
    /// `#[ignore]`d integration test below.
    fn transcribe(&self, samples: &[f32], opts: &TranscribeOpts) -> Result<String, SttError> {
        // Perf instrumentation (issue #115 follow-up): time the whole
        // per-call decode (state creation + full() + segment collection).
        // This is the cost caching does NOT remove — the model load is
        // shared, but every dictation still pays this — so it's the number
        // to watch when judging whether a run "feels" slow. Off unless
        // BLA_PERF_LOG is set; logs sample/duration counts only, never text.
        let transcribe_start = std::time::Instant::now();
        let mut state = self
            .context
            .create_state()
            .map_err(|e| SttError::Transcription(e.to_string()))?;

        let initial_prompt = opts.initial_prompt();
        let mut params =
            whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        // Issue #115 decode tuning: whisper.cpp's own default is
        // `min(4, hardware_concurrency())`, deliberately conservative for a
        // library that doesn't know how it'll be deployed. This engine is
        // always the single, dedicated transcription worker for one
        // dictation at a time (never run concurrently with itself), so
        // using every available core is a straightforward decode-latency
        // win rather than a resource-contention risk. Falls back to 4 (the
        // same number whisper.cpp itself would pick) if the platform can't
        // report a core count.
        let n_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4) as i32;
        params.set_n_threads(n_threads);
        if !initial_prompt.is_empty() {
            params.set_initial_prompt(&initial_prompt);
        }

        state
            .full(params, samples)
            .map_err(|e| SttError::Transcription(e.to_string()))?;

        // Issue #71: `to_str()` (strict UTF-8) used to silently drop any
        // segment it couldn't decode, truncating the transcript with no
        // signal. `to_str_lossy()` never fails to produce text (it replaces
        // invalid byte sequences rather than erroring on them) — the only
        // remaining `Err` is the rare case where the segment's raw text
        // pointer itself couldn't be read. `accumulate_segment_text` is the
        // pure core of this decision (unit-tested above without the
        // `whisper` feature); this loop is the thin, TDD-exempt glue that
        // feeds it real segments.
        let mut text = String::new();
        let mut lossy_segments = 0u32;
        for segment in state.as_iter() {
            let decoded = segment.to_str_lossy().map_err(|_| ());
            if accumulate_segment_text(&mut text, decoded) {
                lossy_segments += 1;
            }
        }
        if lossy_segments > 0 {
            // Data-loss SIGNAL only — never the decoded text itself
            // (MISSION §5/§7: transcript content must never be logged).
            eprintln!(
                "bla[stt]: {lossy_segments} whisper segment(s) contained invalid UTF-8 \
                 and were decoded lossily"
            );
        }
        perf_log(&format!(
            "transcribed {} samples (~{:.1}s audio) in {} ms on {} threads",
            samples.len(),
            samples.len() as f32 / 16_000.0,
            transcribe_start.elapsed().as_millis(),
            n_threads
        ));
        Ok(text.trim().to_string())
    }
}

/// Accumulates one whisper.cpp segment's decoded text into `text` and
/// reports whether that segment required lossy handling (issue #71): a
/// segment whose raw bytes weren't valid UTF-8 (lossily replaced —
/// `Ok(Cow::Owned(_))`) or that couldn't be read at all (`Err`). Previously
/// `WhisperStt::transcribe` used `Segment::to_str()` (strict UTF-8) and
/// silently dropped any segment it returned `Err` for — the transcript was
/// truncated with no signal at all. This is the pure core of the fix: it
/// always contributes whatever text is available (lossily decoded rather
/// than dropped) and returns a bool the caller uses to count/warn, so the
/// fix is unit-tested without needing the `whisper` feature or a real
/// model — `decoded` mirrors exactly what `Segment::to_str_lossy()`
/// returns, minus whisper-rs's own error type, which this module has no
/// reason to depend on outside the feature gate.
fn accumulate_segment_text(
    text: &mut String,
    decoded: Result<std::borrow::Cow<'_, str>, ()>,
) -> bool {
    match decoded {
        Ok(s) => {
            let was_lossy = matches!(s, std::borrow::Cow::Owned(_));
            text.push_str(&s);
            was_lossy
        }
        Err(()) => true,
    }
}

/// Requires a real whisper.cpp model on disk (ADR-0004's app-data model
/// dir); CI has no model file, so this test is `#[ignore]`d and meant to be
/// run manually once you've downloaded a model, e.g.:
///
/// ```sh
/// BLA_TEST_WHISPER_MODEL=/path/to/ggml-model.bin \
///   cargo test --features whisper -- --ignored transcribes_a_real_model
/// ```
#[cfg(all(test, feature = "whisper"))]
mod whisper_integration_tests {
    use super::*;

    #[test]
    #[ignore = "requires a downloaded whisper.cpp model file; not available in CI"]
    fn transcribes_a_real_model() {
        let model_path = std::env::var("BLA_TEST_WHISPER_MODEL")
            .expect("set BLA_TEST_WHISPER_MODEL to a whisper.cpp model file path");
        let stt = WhisperStt::new(&model_path).expect("model should load");
        let samples = vec![0.0_f32; 16_000 * 3]; // 3s of silence
        let text = stt
            .transcribe(&samples, &TranscribeOpts::default())
            .expect("transcription should succeed");
        println!("transcribed: {text:?}");
    }

    /// AC-35 (PRD AC-21, issue #200): the real, whisper-gated half of "a
    /// dictionary term absent from a fixture WAV's default transcription is
    /// correctly recognized once added to the dictionary and injected into
    /// Whisper's `initial_prompt`". `#[ignore]`d for the same reason as
    /// `transcribes_a_real_model` above — no model file (or, here, no
    /// speech fixture) is available in CI — the always-on,
    /// never-`#[ignore]`d dictionary-PLUMBING assertion (does the term
    /// actually reach `TranscribeOpts`/`initial_prompt`) lives in
    /// `lib.rs::dictionary_wiring_tests`, which needs neither a model nor a
    /// WAV file to run.
    ///
    /// `BLA_TEST_DICTIONARY_FIXTURE_WAV` should point at a synthetic
    /// (TTS-generated, per ADR-0007 — never a real recording) 16 kHz mono
    /// WAV containing speech of a term the base model is known to
    /// mis-transcribe without help (an uncommon proper noun/acronym is a
    /// good choice), e.g.:
    ///
    /// ```sh
    /// BLA_TEST_WHISPER_MODEL=/path/to/ggml-model.bin \
    /// BLA_TEST_DICTIONARY_FIXTURE_WAV=/path/to/fixture.wav \
    /// BLA_TEST_DICTIONARY_TERM=Kubernetes \
    ///   cargo test --features whisper -- --ignored dictionary_term_improves_recognition_on_a_real_model
    /// ```
    #[test]
    #[ignore = "requires a downloaded whisper.cpp model file and a speech fixture WAV; not available in CI"]
    fn dictionary_term_improves_recognition_on_a_real_model() {
        let model_path = std::env::var("BLA_TEST_WHISPER_MODEL")
            .expect("set BLA_TEST_WHISPER_MODEL to a whisper.cpp model file path");
        let fixture_path = std::env::var("BLA_TEST_DICTIONARY_FIXTURE_WAV")
            .expect("set BLA_TEST_DICTIONARY_FIXTURE_WAV to a synthetic speech fixture WAV path");
        let term = std::env::var("BLA_TEST_DICTIONARY_TERM")
            .expect("set BLA_TEST_DICTIONARY_TERM to the term the fixture speaks");

        let mut reader = hound::WavReader::open(&fixture_path).expect("fixture WAV should open");
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.expect("sample should decode") as f32 / i16::MAX as f32)
            .collect();

        let stt = WhisperStt::new(&model_path).expect("model should load");

        let without_dictionary = stt
            .transcribe(&samples, &TranscribeOpts::default())
            .expect("transcription without a dictionary should succeed");
        let with_dictionary = stt
            .transcribe(
                &samples,
                &TranscribeOpts {
                    dictionary: vec![term.clone()],
                },
            )
            .expect("transcription with a dictionary should succeed");

        assert!(
            !without_dictionary.contains(&term),
            "test fixture setup: the default transcription already contains {term:?} \
             ({without_dictionary:?}) — pick a fixture/term the base model actually mis-transcribes"
        );
        assert!(
            with_dictionary.contains(&term),
            "AC-35: injecting {term:?} into the dictionary should recover it in the \
             transcription, got {with_dictionary:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // -------------------------------------------------------------
    // Issue #71: WhisperStt::transcribe used to silently drop a segment
    // whose raw bytes weren't valid UTF-8 (`if let Ok(s) = segment.to_str()`
    // -- the `Err` branch just skipped it, with no signal at all). These
    // tests cover accumulate_segment_text, the pure core of the fix: it
    // always contributes whatever text is available -- lossily decoded
    // rather than dropped -- and reports whether that segment needed lossy
    // handling. `decoded` mirrors exactly what whisper-rs's
    // `Segment::to_str_lossy()` returns (`Ok(Cow::Borrowed)` for clean
    // UTF-8, `Ok(Cow::Owned)` when replacement was needed), minus
    // whisper-rs's own error type -- so this is fully unit-tested without
    // the `whisper` feature or a real model.
    // -------------------------------------------------------------

    #[test]
    fn accumulate_segment_text_appends_lossily_decoded_text_instead_of_dropping_it_issue_71() {
        let mut text = String::new();
        let decoded: Result<std::borrow::Cow<'_, str>, ()> = Ok(std::borrow::Cow::Owned(
            "recovered \u{FFFD}text".to_string(),
        ));
        let was_lossy = accumulate_segment_text(&mut text, decoded);
        assert!(
            was_lossy,
            "an owned (lossily-decoded) Cow must be flagged as lossy"
        );
        assert_eq!(
            text, "recovered \u{FFFD}text",
            "issue #71: a lossily-decoded segment's text must still be appended, not dropped"
        );
    }

    #[test]
    fn accumulate_segment_text_flags_but_does_not_panic_on_an_unreadable_segment() {
        let mut text = String::new();
        let was_lossy = accumulate_segment_text(&mut text, Err(()));
        assert!(
            was_lossy,
            "a segment that couldn't be read at all must still be flagged as a loss"
        );
        assert_eq!(
            text, "",
            "nothing to append when the segment couldn't be read at all"
        );
    }

    #[test]
    fn accumulate_segment_text_does_not_flag_clean_utf8_segments() {
        let mut text = String::new();
        let was_lossy = accumulate_segment_text(&mut text, Ok(std::borrow::Cow::Borrowed("clean")));
        assert!(!was_lossy);
        assert_eq!(text, "clean");
    }

    #[test]
    fn accumulate_segment_text_appends_across_multiple_segments_in_order() {
        let mut text = String::new();
        accumulate_segment_text(&mut text, Ok(std::borrow::Cow::Borrowed("hello ")));
        accumulate_segment_text(&mut text, Ok(std::borrow::Cow::Owned("world".to_string())));
        assert_eq!(text, "hello world");
    }

    #[test]
    fn perf_logging_is_off_when_unset_or_explicitly_disabled() {
        // Default posture: a normal run (no BLA_PERF_LOG) stays silent, and
        // an explicit disable ("0" or empty) is honored — so perf output
        // never leaks into an ordinary session.
        assert!(!perf_logging_enabled(None));
        assert!(!perf_logging_enabled(Some("")));
        assert!(!perf_logging_enabled(Some("0")));
    }

    #[test]
    fn perf_logging_is_on_when_set_to_a_non_disable_value() {
        // Any present, non-disable value turns it on — the measurement
        // opt-in (`BLA_PERF_LOG=1`) and forgiving variants.
        assert!(perf_logging_enabled(Some("1")));
        assert!(perf_logging_enabled(Some("true")));
        assert!(perf_logging_enabled(Some("yes")));
    }

    #[test]
    fn empty_terms_yield_empty_prompt() {
        assert_eq!(build_initial_prompt(&[]), "");
    }

    #[test]
    fn single_term_is_rendered_verbatim() {
        assert_eq!(build_initial_prompt(&terms(&["Kubernetes"])), "Kubernetes");
    }

    #[test]
    fn multiple_terms_preserve_input_order() {
        assert_eq!(
            build_initial_prompt(&terms(&["gamma", "alpha", "beta"])),
            "gamma, alpha, beta"
        );
    }

    #[test]
    fn blank_and_whitespace_only_terms_are_dropped() {
        assert_eq!(
            build_initial_prompt(&terms(&["", "   ", "kubectl"])),
            "kubectl"
        );
    }

    #[test]
    fn internal_whitespace_and_newlines_collapse_to_single_spaces() {
        assert_eq!(
            build_initial_prompt(&terms(&["foo\n bar\t baz"])),
            "foo bar baz"
        );
    }

    #[test]
    fn commas_in_terms_are_escaped_so_the_join_stays_unambiguous() {
        assert_eq!(
            build_initial_prompt(&terms(&["Acme, Inc."])),
            "Acme\\, Inc."
        );
    }

    #[test]
    fn backslashes_are_escaped_before_commas_are() {
        assert_eq!(build_initial_prompt(&terms(&["C:\\Users"])), "C:\\\\Users");
    }

    #[test]
    fn length_cap_truncates_at_a_term_boundary_without_exceeding_the_cap() {
        let term = "a".repeat(100);
        let terms: Vec<String> = std::iter::repeat_n(term, 20).collect();
        let prompt = build_initial_prompt(&terms);
        assert!(prompt.len() <= INITIAL_PROMPT_MAX_CHARS);
        for part in prompt.split(", ") {
            assert_eq!(part.len(), 100);
        }
        assert!(!prompt.is_empty());
    }

    // -------------------------------------------------------------
    // Issue #69 (Sentinel, sentinel-pr64-9ced7e6 finding 1): a NUL byte in a
    // dictionary term survives to `CString::new` inside whisper-rs's
    // `set_initial_prompt` and panics there — Rust `String`s can contain
    // interior NULs (they aren't C strings), so `build_initial_prompt` must
    // strip them before any term reaches that call.
    // -------------------------------------------------------------

    #[test]
    fn nul_bytes_inside_a_term_are_stripped_rather_than_reaching_the_rendered_prompt_issue_69() {
        assert_eq!(
            build_initial_prompt(&terms(&["foo\0bar"])),
            "foobar",
            "a NUL byte must be stripped, not passed through to a value \
             whisper-rs's CString::new would reject/panic on"
        );
    }

    #[test]
    fn a_term_consisting_only_of_nul_bytes_is_dropped_like_a_blank_term_issue_69() {
        assert_eq!(
            build_initial_prompt(&terms(&["\0\0\0", "kubectl"])),
            "kubectl"
        );
    }

    #[test]
    fn nul_bytes_do_not_survive_alongside_other_escaping_rules_issue_69() {
        assert_eq!(
            build_initial_prompt(&terms(&["Ac\0me, Inc."])),
            "Acme\\, Inc."
        );
    }

    // -------------------------------------------------------------
    // Issue #70 (Sentinel, sentinel-pr64-9ced7e6 finding 2): once one term
    // overflowed the length cap, `build_initial_prompt` used to `break` out
    // of the loop entirely, silently dropping every subsequent term
    // regardless of whether it would have fit — an order-dependent loss the
    // caller can't predict. It must instead skip the oversized term and
    // keep trying to pack whatever still fits.
    // -------------------------------------------------------------

    #[test]
    fn an_oversized_term_is_skipped_rather_than_dropping_every_later_term_issue_70() {
        let oversized = "x".repeat(INITIAL_PROMPT_MAX_CHARS + 1);
        let prompt = build_initial_prompt(&terms(&[&oversized, "kubectl"]));
        assert_eq!(
            prompt, "kubectl",
            "a later term that fits must survive an earlier oversized term, \
             not be silently dropped by a `break`"
        );
    }

    #[test]
    fn multiple_terms_still_pack_around_a_mid_list_oversized_term_issue_70() {
        let oversized = "x".repeat(INITIAL_PROMPT_MAX_CHARS + 1);
        let prompt = build_initial_prompt(&terms(&["alpha", &oversized, "beta", "gamma"]));
        assert_eq!(prompt, "alpha, beta, gamma");
    }

    #[test]
    fn transcribe_opts_initial_prompt_delegates_to_build_initial_prompt() {
        let opts = TranscribeOpts {
            dictionary: terms(&["foo", "bar"]),
        };
        assert_eq!(
            opts.initial_prompt(),
            build_initial_prompt(&opts.dictionary)
        );
    }

    #[test]
    fn fake_stt_returns_its_canned_transcript_regardless_of_input() {
        let stt = FakeStt::new("hello world");
        let samples = [0.0_f32; 16_000];
        let opts = TranscribeOpts::default();
        assert_eq!(stt.transcribe(&samples, &opts).unwrap(), "hello world");
    }

    #[test]
    fn fake_stt_default_returns_a_nonempty_canned_transcript() {
        let stt = FakeStt::default();
        assert!(!stt
            .transcribe(&[], &TranscribeOpts::default())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn stt_trait_is_object_safe_and_usable_as_a_trait_object() {
        let stt: Box<dyn Stt> = Box::new(FakeStt::new("boxed"));
        assert_eq!(
            stt.transcribe(&[], &TranscribeOpts::default()).unwrap(),
            "boxed"
        );
    }

    #[test]
    fn transcribe_opts_carries_dictionary_terms_through_to_initial_prompt() {
        let opts = TranscribeOpts {
            dictionary: terms(&["Kubernetes", "kubectl"]),
        };
        assert_eq!(opts.initial_prompt(), "Kubernetes, kubectl");
    }
}
