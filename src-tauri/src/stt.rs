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
/// - Blank/whitespace-only terms are dropped.
/// - Internal whitespace (including newlines/tabs) in each term collapses to
///   single spaces, so the prompt stays one line.
/// - Terms are joined in the order given (callers control precedence) with
///   `", "` as the separator.
/// - A literal `\` or `,` inside a term is escaped (`\\`, `\,`) so the join
///   stays unambiguous.
/// - The result is capped at [`INITIAL_PROMPT_MAX_CHARS`] bytes, dropping
///   whole trailing terms rather than truncating one mid-term.
pub fn build_initial_prompt(terms: &[String]) -> String {
    let mut rendered = String::new();

    for term in terms {
        let collapsed = term.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            continue;
        }
        let escaped = collapsed.replace('\\', "\\\\").replace(',', "\\,");

        let separator_len = if rendered.is_empty() { 0 } else { 2 };
        if rendered.len() + separator_len + escaped.len() > INITIAL_PROMPT_MAX_CHARS {
            break;
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
        let context = whisper_rs::WhisperContext::new_with_params(model_path, params)
            .map_err(|e| SttError::ModelLoad(format!("{e} (path: {})", model_path.display())))?;
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

        let mut text = String::new();
        for segment in state.as_iter() {
            if let Ok(s) = segment.to_str() {
                text.push_str(s);
            }
        }
        Ok(text.trim().to_string())
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
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
