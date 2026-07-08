//! Headless dictation pipeline (issue #25, ADR-0002): composes `Stt` +
//! `Cleanup` + the output router into a single [`Pipeline::run`] call, so
//! the whole transcribe -> clean -> route flow is testable end to end
//! without a live mic, clipboard, or network. `audio`'s capture step feeds
//! `Pipeline::run` its `samples`; this module starts one step downstream of
//! that (ADR-0002's module boundary: `audio` stays OS-glue-only).
//!
//! `Pipeline` is generic over its collaborators (`Stt`, `Cleanup`,
//! `Clipboard`, `PasteSynthesizer`, plus an injected `sleep`) so production
//! code and the acceptance suite (`tests/acceptance.rs`) construct the same
//! `Pipeline<...>` type with different concrete pieces — real engines in
//! the app, `FakeStt`/stub transports/no-op OS glue in tests. Cleanup
//! fallback (AC-4, ADR-0005) lives here: if the injected `Cleanup` (e.g.
//! `OllamaCleanup`) returns `CleanupError::Unreachable`, `Pipeline` falls
//! back to `RegexCleanup` and never surfaces the error to the output path.
//!
//! Not yet wired into `commands.rs` / the live Tauri runtime — that's a
//! later step; `dead_code` stays allowed for anything not yet reached from
//! there or from this crate's own tests.
#![allow(dead_code)]

use std::fmt;
use std::time::Duration;

use crate::cleanup::{Cleanup, CleanupError, RegexCleanup, Tone};
use crate::output::{
    self, Clipboard, Clock, OutputMode, OutputOutcome, PasteSynthesizer, RouteError,
};
use crate::stt::{Stt, SttError, TranscribeOpts};

/// Per-run configuration for [`Pipeline::run`].
pub struct PipelineOpts {
    /// Forwarded to `Stt::transcribe` (dictionary `initial_prompt` seam,
    /// AC-21 — empty in M1).
    pub transcribe: TranscribeOpts,
    /// Forwarded to `Cleanup::clean` (`Tone::Verbatim` bypasses cleanup
    /// entirely, mirroring both `RegexCleanup` and `OllamaCleanup`).
    pub tone: Tone,
    /// Which output target `crate::output::route` dispatches to
    /// (cursor-paste or file, AC-3/AC-9).
    pub output_mode: OutputMode,
    /// Injected "now" for file-mode path/timestamp templating (never the
    /// real OS clock — see `output::Clock`).
    pub clock: Clock,
    /// Forwarded to the cursor-paste target's clipboard-restore delay
    /// (ADR-0003); unused by the file target.
    pub restore_delay: Duration,
}

/// What a successful [`Pipeline::run`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The verbatim transcript `Stt::transcribe` returned.
    pub raw_transcript: String,
    /// The transcript after `Cleanup` (or the AC-4 `RegexCleanup` fallback).
    pub cleaned_transcript: String,
    /// True when the configured `Cleanup` returned
    /// `CleanupError::Unreachable` and `Pipeline` fell back to
    /// `RegexCleanup` (AC-4) — never surfaced as an error, but exposed here
    /// for callers/tests that want to observe the fallback happened.
    pub cleanup_fell_back: bool,
    /// What `crate::output::route` did with the cleaned text.
    pub output: OutputOutcome,
}

/// Errors [`Pipeline::run`] can return.
///
/// Deliberately has **no** `Cleanup` variant: per AC-4/ADR-0005, a cleanup
/// backend being unreachable is handled internally (fallback to
/// `RegexCleanup`, recorded in [`Outcome::cleanup_fell_back`]) and never
/// propagates as a pipeline error.
#[derive(Debug)]
pub enum PipelineError {
    /// Transcription failed (see [`SttError`]).
    Stt(SttError),
    /// Routing the cleaned text to its output target failed (see
    /// [`RouteError`]) — e.g. a file-mode path that escapes its
    /// confinement base.
    Output(RouteError),
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PipelineError::Stt(e) => write!(f, "pipeline transcription failed: {e}"),
            PipelineError::Output(e) => write!(f, "pipeline output routing failed: {e:?}"),
        }
    }
}

impl std::error::Error for PipelineError {}

/// The headless dictation pipeline: `Stt::transcribe` -> `Cleanup::clean`
/// (with the AC-4 `RegexCleanup` fallback) -> `crate::output::route`.
///
/// Generic over every collaborator so the exact same type assembles both
/// the real, OS-touching pipeline and a fully-stubbed one for tests
/// (ADR-0002's headless-acceptance-suite requirement, AC-1/AC-2/AC-4/AC-5).
pub struct Pipeline<S, C, Clip, Paste, Sleep>
where
    S: Stt,
    C: Cleanup,
    Clip: Clipboard,
    Paste: PasteSynthesizer,
    Sleep: Fn(Duration),
{
    stt: S,
    cleanup: C,
    clipboard: Clip,
    paste: Paste,
    sleep: Sleep,
}

impl<S, C, Clip, Paste, Sleep> Pipeline<S, C, Clip, Paste, Sleep>
where
    S: Stt,
    C: Cleanup,
    Clip: Clipboard,
    Paste: PasteSynthesizer,
    Sleep: Fn(Duration),
{
    /// Assembles a pipeline from its collaborators. `cleanup` is the
    /// *primary* cleanup pass (e.g. `OllamaCleanup` in production, or
    /// `RegexCleanup` directly when no LLM-backed pass is wanted); the AC-4
    /// fallback to `RegexCleanup` is always available regardless of what's
    /// passed here. `sleep` is the restore-delay sleep the cursor-paste
    /// output target uses (injected so tests never actually wait).
    pub fn new(stt: S, cleanup: C, clipboard: Clip, paste: Paste, sleep: Sleep) -> Self {
        Self {
            stt,
            cleanup,
            clipboard,
            paste,
            sleep,
        }
    }

    /// Runs the full pipeline over `samples` (16 kHz mono `f32`, the format
    /// `audio` produces): transcribe, clean (falling back to `RegexCleanup`
    /// on `CleanupError::Unreachable` — AC-4), then route the cleaned text
    /// per `opts.output_mode` (AC-3/AC-9).
    pub fn run(&self, samples: &[f32], opts: &PipelineOpts) -> Result<Outcome, PipelineError> {
        let raw_transcript = self
            .stt
            .transcribe(samples, &opts.transcribe)
            .map_err(PipelineError::Stt)?;

        let (cleaned_transcript, cleanup_fell_back) =
            self.clean_with_fallback(&raw_transcript, opts.tone);

        let output = output::route(
            &opts.output_mode,
            cleaned_transcript.clone(),
            opts.clock,
            &self.clipboard,
            &self.paste,
            |delay| (self.sleep)(delay),
            opts.restore_delay,
        )
        .map_err(PipelineError::Output)?;

        Ok(Outcome {
            raw_transcript,
            cleaned_transcript,
            cleanup_fell_back,
            output,
        })
    }

    /// AC-4 / ADR-0005: try the configured `Cleanup`; on
    /// `CleanupError::Unreachable`, fall back to `RegexCleanup` (always
    /// infallible), so no cleanup error ever reaches the output path.
    /// Returns the cleaned text plus whether the fallback fired.
    fn clean_with_fallback(&self, raw: &str, tone: Tone) -> (String, bool) {
        match self.cleanup.clean(raw, tone) {
            Ok(text) => (text, false),
            Err(CleanupError::Unreachable) => {
                let text = RegexCleanup
                    .clean(raw, tone)
                    .expect("RegexCleanup is infallible");
                (text, true)
            }
        }
    }
}
