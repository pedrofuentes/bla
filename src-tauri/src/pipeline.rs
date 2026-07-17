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
use crate::snippets;
use crate::store::Snippet;
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
    /// Issue #263 (AC-53), part of #242's M4 scope: the caller's currently
    /// configured snippets (typically `Store::list_snippets`'s result,
    /// read fresh per dictation — never cached — mirroring how `tone`
    /// above is resolved from `list_tone_rules` on every run rather than
    /// once at startup). `Pipeline::run` checks these against the RAW
    /// transcript, before `tone` is ever consulted for a `Cleanup` call —
    /// see `run`'s own doc comment for why raw-transcript matching was
    /// chosen over PRD.md's older cleaned-transcript wording.
    pub snippets: Vec<Snippet>,
}

/// What a successful [`Pipeline::run`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The verbatim transcript `Stt::transcribe` returned.
    pub raw_transcript: String,
    /// The transcript after `Cleanup` (or the AC-4 `RegexCleanup` fallback).
    pub cleaned_transcript: String,
    /// True when `Pipeline` fell back to `RegexCleanup` instead of using the
    /// configured `Cleanup`'s output — either because it returned
    /// `CleanupError::Unreachable` (AC-4) OR because its `Ok` output looked
    /// like a conversational preamble / prompt echo rather than a rewrite
    /// (issue #283; see [`Pipeline::clean_with_fallback`]). Never surfaced as
    /// an error, but exposed here for callers/tests that want to observe the
    /// fallback happened (e.g. the `should_settle_with_notice` tray notice).
    pub cleanup_fell_back: bool,
    /// Issue #263 (AC-53): true when a configured snippet's trigger
    /// matched the raw transcript and `cleaned_transcript` is therefore
    /// that snippet's stored `body` rather than `Cleanup`'s output — in
    /// which case `Cleanup::clean` (and any AC-4 fallback) was never
    /// invoked at all for this run. Always `false` when `cleanup_fell_back`
    /// is `true`: the two are mutually exclusive outcomes of the same
    /// branch in `run`.
    pub snippet_matched: bool,
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
    /// `audio` produces): transcribe, then either short-circuit on a
    /// snippet match (AC-53) or clean (falling back to `RegexCleanup` on
    /// `CleanupError::Unreachable` — AC-4), then route the resulting text
    /// per `opts.output_mode` (AC-3/AC-9).
    ///
    /// Issue #263 / AC-53 (part of #242's M4 scope): `opts.snippets` is
    /// checked against the RAW transcript via `snippets::match_snippet`
    /// BEFORE `self.cleanup` is ever consulted. On a match, `self.cleanup`
    /// (and its AC-4 `RegexCleanup` fallback) is skipped entirely for this
    /// run — the matched snippet's stored body is routed to output
    /// directly. This deliberately follows #242's cofounder-approved
    /// scope (raw-transcript match, pre-cleanup short-circuit) rather than
    /// PRD.md's older AC-24 wording (cleaned-transcript match, post-cleanup
    /// expansion); see this crate's #263 PR description for that flagged
    /// documentation conflict.
    pub fn run(&self, samples: &[f32], opts: &PipelineOpts) -> Result<Outcome, PipelineError> {
        let raw_transcript = self
            .stt
            .transcribe(samples, &opts.transcribe)
            .map_err(PipelineError::Stt)?;

        let (cleaned_transcript, cleanup_fell_back, snippet_matched) =
            match snippets::match_snippet(&raw_transcript, &opts.snippets) {
                Some(body) => (body, false, true),
                None => {
                    let (cleaned, fell_back) = self.clean_with_fallback(&raw_transcript, opts.tone);
                    (cleaned, fell_back, false)
                }
            };

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
            snippet_matched,
            output,
        })
    }

    /// AC-4 / ADR-0005 (+ issue #283): try the configured `Cleanup`; fall
    /// back to `RegexCleanup` (always infallible) — and record that it fell
    /// back — in either of two cases, so no cleanup error and no polluted
    /// model output ever reaches the output path:
    ///
    /// 1. `CleanupError::Unreachable` (AC-4): the LLM backend was
    ///    unreachable/timed out.
    /// 2. The LLM returned `Ok`, but the text looks like a conversational
    ///    preamble / prompt echo rather than the rewritten transcript (issue
    ///    #283, ac7-p0: e.g. "This is a formal rewrite of your original
    ///    transcript: …"). Emitting that as the "cleaned" dictation is worse
    ///    than the deterministic regex baseline, so degrade to the baseline —
    ///    the same safe-degradation contract as the unreachable path. The
    ///    detector (`preamble::looks_like_preamble`) is conservative: a
    ///    legitimate rewrite that merely starts with "This" is not caught.
    ///
    /// Returns the cleaned text plus whether the fallback fired.
    fn clean_with_fallback(&self, raw: &str, tone: Tone) -> (String, bool) {
        match self.cleanup.clean(raw, tone) {
            Ok(text) if crate::preamble::looks_like_preamble(&text) => {
                let baseline = RegexCleanup
                    .clean(raw, tone)
                    .expect("RegexCleanup is infallible");
                (baseline, true)
            }
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
