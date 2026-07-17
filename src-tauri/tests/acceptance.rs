//! Cumulative acceptance suite (issue #25): exercises the headless
//! dictation pipeline (`bla_lib::pipeline::Pipeline`) end to end, entirely
//! from injected fakes/stubs — no live mic, clipboard, model, or network.
//!
//! Every test fixture here is synthetic (ADR-0007): audio "samples" are
//! either silent or an in-code synthesized tone (never a real recording),
//! and every transcript is a literal string written for this suite.
//!
//! Test names are bound to stable AC ids (`ac1_`, `ac2_`, `ac4_`, `ac5_`,
//! `ac24_`, `ac53_`) so later milestones extend this same suite rather than
//! re-numbering it.
//! AC-3 (file-mode templating) and AC-9 (clipboard restore) already have
//! their own coverage in `output.rs`'s unit tests; this suite's file-mode
//! usage below only stands in as a network-free, OS-glue-free output
//! target so `Pipeline::run` can be driven end to end.

use std::io;
use std::time::Duration;

use bla_lib::cleanup::{
    Cleanup, CleanupError, NoRealNetworkTransport, OllamaCleanup, RegexCleanup, StubTransport,
    Tone, TransportError,
};
use bla_lib::errors::{self, ErrorKind, PipelineErrorEvent};
use bla_lib::output::{Clipboard, Clock, FileConfig, OutputMode, OutputOutcome, PasteSynthesizer};
use bla_lib::pipeline::{Pipeline, PipelineOpts};
use bla_lib::store::Snippet;
use bla_lib::stt::{FakeStt, Stt, SttError, TranscribeOpts};

// `StubTransport` (a preprogrammed-outcome `OllamaTransport` that never
// touches a real socket) is imported from `bla_lib::cleanup` rather than
// defined locally: it carries the sealed `NoRealNetworkTransport` marker
// (issue #86) the AC-5 test below asserts against, which only `cleanup.rs`
// itself can grant.

/// Encodes `model_output` as the Ollama `/api/generate` JSON body
/// `OllamaCleanup` expects to parse.
fn ollama_response_body(model_output: &str) -> String {
    serde_json::json!({ "response": model_output, "done": true }).to_string()
}

/// No-op `Clipboard`: this suite only routes to the file target (network-
/// and OS-glue-free), so these methods are never actually exercised, but
/// `Pipeline` needs a `Clipboard` to construct.
struct NoopClipboard;

impl Clipboard for NoopClipboard {
    fn get(&self) -> io::Result<String> {
        Ok(String::new())
    }
    fn set(&self, _contents: &str) -> io::Result<()> {
        Ok(())
    }
}

/// No-op `PasteSynthesizer` — see [`NoopClipboard`].
struct NoopPaste;

impl PasteSynthesizer for NoopPaste {
    fn synthesize_paste(&self) -> io::Result<()> {
        Ok(())
    }
}

fn fixed_clock() -> Clock {
    Clock {
        year: 2026,
        month: 7,
        day: 8,
        hour: 9,
        minute: 0,
    }
}

/// A file-mode output target confined to a fresh temp dir — deterministic,
/// no clipboard/paste, no network.
fn file_output_mode(dir: &tempfile::TempDir) -> OutputMode {
    OutputMode::File {
        base_dir: dir.path().to_path_buf(),
        config: FileConfig {
            path_template: "dictation.md".to_string(),
            timestamp_prefix_template: None,
        },
    }
}

#[test]
fn ac1_headless_pipeline_removes_fillers_and_applies_self_correction() {
    // AC-1: a raw transcript with fillers AND one self-correction, run
    // through capture-decode (stubbed via FakeStt) -> whisper (stubbed) ->
    // cleanup, must produce text with NO filler words and the corrected
    // phrase. Per ADR-0005, RegexCleanup alone can't resolve a
    // self-correction — that repair is the LLM path — so this drives
    // OllamaCleanup backed by a stub transport that returns the
    // cleaned-and-corrected text a real model would produce.
    let raw_transcript = "um so let's meet Tuesday, actually Wednesday, you know";
    let stt = FakeStt::new(raw_transcript);

    let corrected = "Let's meet Wednesday.";
    let transport = StubTransport {
        response: Ok(ollama_response_body(corrected)),
    };
    let cleanup = OllamaCleanup::new("http://localhost:11434", "llama3", transport);

    let dir = tempfile::tempdir().expect("tempdir");
    let pipeline = Pipeline::new(
        stt,
        cleanup,
        NoopClipboard,
        NoopPaste,
        |_delay: Duration| {},
    );

    let opts = PipelineOpts {
        transcribe: TranscribeOpts::default(),
        tone: Tone::Neutral,
        output_mode: file_output_mode(&dir),
        clock: fixed_clock(),
        restore_delay: Duration::from_millis(0),
        snippets: vec![],
    };

    let outcome = pipeline
        .run(&[0.0_f32; 1_600], &opts)
        .expect("AC-1: pipeline run should succeed");

    assert_eq!(outcome.raw_transcript, raw_transcript);
    assert_eq!(outcome.cleaned_transcript, corrected);
    assert!(
        !outcome.cleaned_transcript.to_lowercase().contains("um"),
        "AC-1: cleaned output must have no filler words, got {:?}",
        outcome.cleaned_transcript
    );
    assert!(
        !outcome
            .cleaned_transcript
            .to_lowercase()
            .contains("you know"),
        "AC-1: cleaned output must have no filler words, got {:?}",
        outcome.cleaned_transcript
    );
    assert!(
        outcome.cleaned_transcript.contains("Wednesday"),
        "AC-1: cleaned output must contain the corrected phrase"
    );
    assert!(
        !outcome.cleaned_transcript.contains("Tuesday"),
        "AC-1: cleaned output must not contain the superseded phrase"
    );
    assert!(
        !outcome.cleanup_fell_back,
        "AC-1: the LLM path must be used, not the fallback"
    );
}

#[test]
fn ac2_latency_budget_regex_path_under_2s_for_15s_fixture() {
    // AC-2: pipeline (transcribe + cleanup, regex path) for a 15-second,
    // 16 kHz fixture completes in under 2s. FakeStt stands in for
    // transcription (real whisper-rs latency on a 15s clip is validated
    // separately: locally under `cargo test --features whisper` against a
    // downloaded model, and at the milestone's AC-7 human smoke test — CI
    // has no model file to time against, per ADR-0004/stt.rs). What this
    // benchmarks is the pipeline's own overhead (data handling + the regex
    // cleanup path) at AC-2's target input size.
    let raw_transcript =
        "um so let's meet, uh, at the office, like, tomorrow, you know, around three pm";
    let stt = FakeStt::new(raw_transcript);
    let cleanup = RegexCleanup;

    let dir = tempfile::tempdir().expect("tempdir");
    let pipeline = Pipeline::new(
        stt,
        cleanup,
        NoopClipboard,
        NoopPaste,
        |_delay: Duration| {},
    );

    // 15-second-equivalent fixture at the 16 kHz mono format `audio`
    // produces (`audio::TARGET_SAMPLE_RATE`) — synthetic silence; FakeStt
    // ignores sample content, so only the byte volume matters here.
    let fifteen_seconds = 15 * bla_lib::audio::TARGET_SAMPLE_RATE as usize;
    let samples = vec![0.0_f32; fifteen_seconds];

    let opts = PipelineOpts {
        transcribe: TranscribeOpts::default(),
        tone: Tone::Neutral,
        output_mode: file_output_mode(&dir),
        clock: fixed_clock(),
        restore_delay: Duration::from_millis(0),
        snippets: vec![],
    };

    let start = std::time::Instant::now();
    let outcome = pipeline
        .run(&samples, &opts)
        .expect("AC-2: pipeline run should succeed");
    let elapsed = start.elapsed();

    // Logged per run per AC-2's requirement.
    println!(
        "AC-2 latency (regex path, FakeStt, 15s-equivalent / {} samples): {elapsed:?}",
        samples.len()
    );

    assert!(
        elapsed < Duration::from_secs(2),
        "AC-2 budget exceeded: {elapsed:?} >= 2s"
    );
    assert!(!outcome.cleaned_transcript.is_empty());
    assert!(!outcome.cleanup_fell_back);
}

#[test]
fn ac4_ollama_unreachable_falls_back_to_regex_cleanup_with_no_error() {
    // AC-4: with Ollama unreachable, the pipeline must still return
    // rule-cleaned text with no error surfaced to the output path.
    let raw_transcript = "um, hello world";
    let stt = FakeStt::new(raw_transcript);

    let transport = StubTransport {
        response: Err(TransportError::ConnectionFailed),
    };
    let cleanup = OllamaCleanup::new("http://localhost:11434", "llama3", transport);

    let dir = tempfile::tempdir().expect("tempdir");
    let pipeline = Pipeline::new(
        stt,
        cleanup,
        NoopClipboard,
        NoopPaste,
        |_delay: Duration| {},
    );

    let opts = PipelineOpts {
        transcribe: TranscribeOpts::default(),
        tone: Tone::Neutral,
        output_mode: file_output_mode(&dir),
        clock: fixed_clock(),
        restore_delay: Duration::from_millis(0),
        snippets: vec![],
    };

    let outcome = pipeline
        .run(&[0.0_f32; 1_600], &opts)
        .expect("AC-4: no error must surface to the output path when Ollama is unreachable");

    assert_eq!(
        outcome.cleaned_transcript, "Hello world.",
        "AC-4: fallback must produce RegexCleanup's normal output"
    );
    assert!(
        outcome.cleanup_fell_back,
        "AC-4: the fallback path must have fired"
    );
}

#[test]
fn ac5_full_pipeline_run_makes_no_network_io_outside_allowlist() {
    // AC-5: a full pipeline run must perform no runtime network I/O
    // outside MISSION §5's allowlist (huggingface.co model download,
    // localhost:11434 Ollama). This run is constructed entirely from
    // injected stubs: FakeStt (pure in-memory), a StubTransport-backed
    // OllamaCleanup (see the compile-time guard below), and no-op
    // clipboard/paste — none of which perform any I/O, let alone open a
    // socket, so this run opens zero sockets by construction.
    //
    // The only component in this crate that can ever touch a real socket
    // is `UreqTransport` (cleanup.rs), and it is never constructed here.
    // Issue #86: this is a REAL type-level guard, not the tautological
    // `assert_type_ne_all!(StubTransport, UreqTransport)` it replaces (any
    // two distinct named types are always "not equal" to that macro, so it
    // could never actually catch a real transport being substituted in —
    // it just restated that the two type names differ). `StubTransport`
    // carries the sealed `NoRealNetworkTransport` marker that only
    // `cleanup.rs` can grant, and `UreqTransport` deliberately does not
    // implement it — so this line fails to COMPILE if a future edit ever
    // swapped the stub for the real transport in this test, guarding
    // against silently reintroducing real network I/O into what must stay
    // a network-free acceptance case.
    fn assert_no_real_network_transport<T: NoRealNetworkTransport>() {}
    assert_no_real_network_transport::<StubTransport>();

    let raw_transcript = "um, hello there, you know";
    let stt = FakeStt::new(raw_transcript);

    let transport = StubTransport {
        response: Ok(ollama_response_body("Hello there.")),
    };
    let cleanup = OllamaCleanup::new("http://localhost:11434", "llama3", transport);

    let dir = tempfile::tempdir().expect("tempdir");
    let pipeline = Pipeline::new(
        stt,
        cleanup,
        NoopClipboard,
        NoopPaste,
        |_delay: Duration| {},
    );

    let opts = PipelineOpts {
        transcribe: TranscribeOpts::default(),
        tone: Tone::Neutral,
        output_mode: file_output_mode(&dir),
        clock: fixed_clock(),
        restore_delay: Duration::from_millis(0),
        snippets: vec![],
    };

    let outcome = pipeline
        .run(&[0.0_f32; 1_600], &opts)
        .expect("AC-5: pipeline run should succeed with zero real network I/O");

    assert_eq!(outcome.cleaned_transcript, "Hello there.");
}

// -------------------------------------------------------------
// Issue #126, M2 PR 2.4: typed pipeline-error events (bla_lib::errors)
// -------------------------------------------------------------

/// A fake `Stt` whose error message IS a fixture "transcript" — simulating
/// the worst case where a native error happens to echo dictated content back
/// (e.g. a transcription engine's error string). Only used by
/// `pipeline_error_kind_mapping_never_leaks_transcript_text` below.
struct FailingStt {
    error_message: String,
}

impl Stt for FailingStt {
    fn transcribe(&self, _samples: &[f32], _opts: &TranscribeOpts) -> Result<String, SttError> {
        Err(SttError::Transcription(self.error_message.clone()))
    }
}

#[test]
fn pipeline_error_kind_mapping_never_leaks_transcript_text() {
    // HARD RULE (issue #126, errors.rs module doc): the `pipeline-error`
    // event payload must never carry transcript/clipboard/audio content,
    // even if the underlying error's own message happens to embed it.
    // FailingStt's error message IS the fixture transcript below, so this
    // proves the mapping derives `message` purely from the ErrorKind
    // variant, never from the source error's own text.
    let fixture_transcript =
        "the secret dictated sentence that must never reach a pipeline-error toast";
    let stt = FailingStt {
        error_message: fixture_transcript.to_string(),
    };
    let cleanup = RegexCleanup;

    let dir = tempfile::tempdir().expect("tempdir");
    let pipeline = Pipeline::new(
        stt,
        cleanup,
        NoopClipboard,
        NoopPaste,
        |_delay: Duration| {},
    );

    let opts = PipelineOpts {
        transcribe: TranscribeOpts::default(),
        tone: Tone::Neutral,
        output_mode: file_output_mode(&dir),
        clock: fixed_clock(),
        restore_delay: Duration::from_millis(0),
        snippets: vec![],
    };

    let err = pipeline
        .run(&[0.0_f32; 1_600], &opts)
        .expect_err("FailingStt must surface a pipeline error");

    let kind = errors::error_kind_for_pipeline_error(&err);
    let event = PipelineErrorEvent::from(&kind);
    let serialized = serde_json::to_string(&event).expect("event must serialize");

    assert!(
        !serialized.contains(fixture_transcript),
        "pipeline-error payload leaked the wrapped error's text: {serialized}"
    );
    assert!(!event.message.contains(fixture_transcript));
    assert!(kind.is_blocking(), "a transcription failure is blocking");
}

#[test]
fn ac4b_ollama_unreachable_still_pastes_and_is_informational_not_blocking() {
    // Extends ac4 (above) without touching its assertions: the AC-4
    // Ollama-unreachable fallback is informational, not blocking —
    // `lib.rs::run_pipeline_in_background` emits
    // `ErrorKind::OllamaUnreachable` alongside `set_pipeline_state(.., Idle)`
    // on this path, never `Error`, and the dictation still completes and
    // pastes/writes fully.
    let raw_transcript = "um, hello world";
    let stt = FakeStt::new(raw_transcript);

    let transport = StubTransport {
        response: Err(TransportError::ConnectionFailed),
    };
    let cleanup = OllamaCleanup::new("http://localhost:11434", "llama3", transport);

    let dir = tempfile::tempdir().expect("tempdir");
    let pipeline = Pipeline::new(
        stt,
        cleanup,
        NoopClipboard,
        NoopPaste,
        |_delay: Duration| {},
    );

    let opts = PipelineOpts {
        transcribe: TranscribeOpts::default(),
        tone: Tone::Neutral,
        output_mode: file_output_mode(&dir),
        clock: fixed_clock(),
        restore_delay: Duration::from_millis(0),
        snippets: vec![],
    };

    let outcome = pipeline
        .run(&[0.0_f32; 1_600], &opts)
        .expect("AC-4: no error must surface to the output path when Ollama is unreachable");

    assert!(
        outcome.cleanup_fell_back,
        "AC-4: the fallback path must have fired"
    );

    // The dictation still completed fully — output was written (this
    // suite's file-mode stand-in for "pasted"), not blocked by the
    // informational kind lib.rs emits alongside this outcome.
    match outcome.output {
        OutputOutcome::AppendedTo(_) => {}
        other => panic!("expected the file target to have been written, got {other:?}"),
    }

    let kind = ErrorKind::OllamaUnreachable;
    assert!(
        !kind.is_blocking(),
        "AC-4: Ollama-unreachable must be informational, not blocking"
    );
    let event = PipelineErrorEvent::from(&kind);
    assert_eq!(event.kind, "OllamaUnreachable");
}

// -------------------------------------------------------------
// Issue #263 (AC-53, AC-24): snippet trigger matching wired into
// `Pipeline::run`, short-circuiting straight to output BEFORE any
// `Cleanup::clean` call. Per #242's cofounder-approved M4 scope (followed
// here over PRD.md's now-superseded AC-24 wording — see this PR's
// description for the flagged doc conflict), the match runs against the
// RAW transcript, not cleaned text.
// -------------------------------------------------------------

/// Synthetic fixture constructor (ADR-0007) — every field made up, never
/// real dictation text. Mirrors `snippets.rs`'s own test-module helper of
/// the same name/shape.
fn snippet(id: i64, trigger: &str, body: &str) -> Snippet {
    Snippet {
        id,
        trigger: trigger.to_string(),
        body: body.to_string(),
        created_at_ms: 1_000 + id,
    }
}

/// A `Cleanup` that fails the test the instant `clean` is invoked — the
/// AC-53 enforcement stub. Proves the short-circuit itself (Cleanup is
/// never even asked), not merely that the right text happened to come out
/// the other end.
struct PanicIfCalledCleanup;

impl Cleanup for PanicIfCalledCleanup {
    fn clean(&self, _raw: &str, _tone: Tone) -> Result<String, CleanupError> {
        panic!(
            "AC-53: Cleanup::clean must never be invoked when a snippet trigger matched the raw \
             transcript"
        );
    }
}

#[test]
fn ac53_snippet_match_short_circuits_before_cleanup_is_ever_invoked() {
    let raw_transcript = "please add my sig now";
    let stt = FakeStt::new(raw_transcript);

    let snippets = vec![snippet(1, "sig", "Best regards,\nPat Nguyen")];

    let dir = tempfile::tempdir().expect("tempdir");
    // `PanicIfCalledCleanup`, not `RegexCleanup`/`OllamaCleanup`: if the
    // pipeline ever reached `Cleanup::clean` on this run (fallback path
    // included), this whole test would panic rather than merely assert the
    // wrong text.
    let pipeline = Pipeline::new(
        stt,
        PanicIfCalledCleanup,
        NoopClipboard,
        NoopPaste,
        |_delay: Duration| {},
    );

    let opts = PipelineOpts {
        transcribe: TranscribeOpts::default(),
        tone: Tone::Neutral,
        output_mode: file_output_mode(&dir),
        clock: fixed_clock(),
        restore_delay: Duration::from_millis(0),
        snippets,
    };

    let outcome = pipeline
        .run(&[0.0_f32; 1_600], &opts)
        .expect("AC-53: pipeline run should succeed via the snippet short-circuit");

    assert_eq!(outcome.raw_transcript, raw_transcript);
    assert_eq!(outcome.cleaned_transcript, "Best regards,\nPat Nguyen");
    assert!(
        outcome.snippet_matched,
        "AC-53: Outcome must record that the short-circuit fired"
    );
    assert!(
        !outcome.cleanup_fell_back,
        "AC-53: this is the snippet short-circuit, not the AC-4 Cleanup fallback"
    );

    match outcome.output {
        OutputOutcome::AppendedTo(_) => {}
        other => panic!("expected the file target to have been written, got {other:?}"),
    }
}

#[test]
fn ac24_configured_snippet_trigger_expands_to_its_configured_text() {
    // AC-24 (PRD.md): a configured snippet trigger phrase present in the
    // transcript expands to its configured text. Unlike ac53 above (which
    // proves the short-circuit itself with a panicking stub), this drives
    // an ORDINARY `Cleanup` (`RegexCleanup`) to prove snippet expansion
    // also behaves correctly wired into a realistic pipeline configuration.
    let raw_transcript = "please send my addr to the team";
    let stt = FakeStt::new(raw_transcript);

    let snippets = vec![
        snippet(1, "addr", "123 Main St, Springfield"),
        snippet(2, "sig", "Best, Pat"),
    ];

    let dir = tempfile::tempdir().expect("tempdir");
    let pipeline = Pipeline::new(
        stt,
        RegexCleanup,
        NoopClipboard,
        NoopPaste,
        |_delay: Duration| {},
    );

    let opts = PipelineOpts {
        transcribe: TranscribeOpts::default(),
        tone: Tone::Neutral,
        output_mode: file_output_mode(&dir),
        clock: fixed_clock(),
        restore_delay: Duration::from_millis(0),
        snippets,
    };

    let outcome = pipeline
        .run(&[0.0_f32; 1_600], &opts)
        .expect("AC-24: pipeline run should succeed");

    assert_eq!(outcome.cleaned_transcript, "123 Main St, Springfield");
    assert!(outcome.snippet_matched);
}

#[test]
fn ac24_a_transcript_with_no_matching_trigger_falls_through_to_ordinary_cleanup() {
    // Contrast case, proving the ac24 test above is actually discriminating:
    // a transcript that matches no configured trigger must fall through to
    // ordinary `Cleanup` behavior, not silently produce some snippet body
    // anyway.
    let raw_transcript = "um, what time is the meeting";
    let stt = FakeStt::new(raw_transcript);

    let snippets = vec![snippet(1, "sig", "Best, Pat")];

    let dir = tempfile::tempdir().expect("tempdir");
    let pipeline = Pipeline::new(
        stt,
        RegexCleanup,
        NoopClipboard,
        NoopPaste,
        |_delay: Duration| {},
    );

    let opts = PipelineOpts {
        transcribe: TranscribeOpts::default(),
        tone: Tone::Neutral,
        output_mode: file_output_mode(&dir),
        clock: fixed_clock(),
        restore_delay: Duration::from_millis(0),
        snippets,
    };

    let outcome = pipeline
        .run(&[0.0_f32; 1_600], &opts)
        .expect("pipeline run should succeed");

    assert_eq!(outcome.cleaned_transcript, "What time is the meeting.");
    assert!(!outcome.snippet_matched);
}

#[test]
fn ac_cleanup_preamble_output_falls_back_to_regex_issue_283() {
    // Issue #283 (ac7-p0): even when Ollama is REACHABLE, the model
    // (hardcoded llama3) sometimes returns a conversational preamble/label
    // instead of only the rewritten transcript — e.g. "This is a formal
    // rewrite of your original transcript: This Is Normal." The pipeline
    // must detect that polluted output and fall back to the deterministic
    // regex baseline (recording `cleanup_fell_back`), never emitting the
    // preamble to the output path.
    let raw_transcript = "this is normal";
    let stt = FakeStt::new(raw_transcript);

    let polluted = "This is a formal rewrite of your original transcript: This Is Normal.";
    let transport = StubTransport {
        response: Ok(ollama_response_body(polluted)),
    };
    let cleanup = OllamaCleanup::new("http://localhost:11434", "llama3", transport);

    let dir = tempfile::tempdir().expect("tempdir");
    let pipeline = Pipeline::new(stt, cleanup, NoopClipboard, NoopPaste, |_d: Duration| {});

    let opts = PipelineOpts {
        transcribe: TranscribeOpts::default(),
        tone: Tone::Formal,
        output_mode: file_output_mode(&dir),
        clock: fixed_clock(),
        restore_delay: Duration::from_millis(0),
        snippets: vec![],
    };

    let outcome = pipeline
        .run(&[0.0_f32; 1_600], &opts)
        .expect("issue #283: a preamble-polluted response must not surface an error");

    assert_eq!(
        outcome.cleaned_transcript, "This is normal.",
        "issue #283: the regex baseline output must be used, not the model's preamble"
    );
    assert!(
        !outcome
            .cleaned_transcript
            .to_lowercase()
            .contains("formal rewrite"),
        "issue #283: the conversational preamble must never reach the output path, got {:?}",
        outcome.cleaned_transcript
    );
    assert!(
        outcome.cleanup_fell_back,
        "issue #283: the safe regex-baseline fallback must have fired"
    );
}
