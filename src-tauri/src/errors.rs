//! Typed pipeline-error events (issue #126, M2 PR 2.4): a small, closed
//! vocabulary the UI toasts on ([`crate::lib`]'s `pipeline-error` Tauri
//! event), mapped purely from the crate's existing error types
//! ([`crate::pipeline::PipelineError`], [`crate::audio::CaptureError`], the
//! `lib.rs::build_stt` model-missing String surface).
//!
//! **HARD RULE** (MISSION §7 no-log invariant, extended to IPC events): the
//! `message` emitted to the UI is always static and derived from the *kind*
//! ([`ErrorKind::message`]), never from the wrapped error's own text — so a
//! fault whose underlying error happens to embed transcript/clipboard/audio
//! content can never leak it to the frontend. The `error_kind_for_*`
//! mapping functions below only ever branch on which *variant* fired; none
//! of them read `.to_string()`/`Display` of the source error into the
//! outgoing [`ErrorKind::Other`] message. A test below asserts this by
//! feeding a fake error that wraps a fixture "transcript" string and
//! checking the serialized event never contains it.
//!
//! **Semantics** (issue #126 kickoff): [`ErrorKind::OllamaUnreachable`] is
//! informational — the AC-4/ADR-0005 `RegexCleanup` fallback still pastes,
//! so this kind is emitted *alongside* a successful pipeline outcome, never
//! in place of one. [`ErrorKind::HistoryPersistFailed`] (issue #220) is
//! likewise informational — the dictation already pasted/wrote
//! successfully; only the secondary `Store::insert_history` write failed.
//! [`ErrorKind::ModelMissing`] and [`ErrorKind::MicPermissionDenied`] are
//! blocking: the pipeline could not run this dictation at all.
//! [`ErrorKind::is_blocking`] captures that distinction for callers (the
//! toast UI, `lib.rs`'s emit sites).

use serde::Serialize;

use crate::audio::CaptureError;
use crate::pipeline::PipelineError;
use crate::stt::SttError;

/// A closed, UI-facing vocabulary for what can go wrong in the dictation
/// pipeline (issue #126). See the module doc for blocking-vs-informational
/// semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ErrorKind {
    /// The Whisper model file is missing, corrupt, or failed to load
    /// (`SttError::ModelLoad`, or `lib.rs::build_stt`'s "no engine
    /// available" surface). Blocking.
    ModelMissing,
    /// The configured Ollama cleanup backend was unreachable, so `Pipeline`
    /// fell back to `RegexCleanup` (AC-4/ADR-0005). Informational — the
    /// dictation still completed and pasted.
    OllamaUnreachable,
    /// The microphone could not be opened at capture-start
    /// (`audio::CaptureError`) — most commonly OS mic-permission denial.
    /// Blocking.
    MicPermissionDenied,
    /// A completed dictation's history row failed to persist
    /// (`Store::insert_history`, issue #220) — the dictation itself already
    /// succeeded (pasted/written) by the time this can fire, so this is
    /// informational, not a pipeline failure. Carries no data from the
    /// underlying `rusqlite::Error` (HARD RULE, module doc) — not even a
    /// sanitized code: every other zero-payload kind here (this one
    /// included) already conveys everything the user needs to know via its
    /// fixed `message`, and a SQLite error code is not itself user content
    /// but still isn't worth a second wire field for a toast this generic.
    HistoryPersistFailed,
    /// Anything else. `message` is always a fixed, kind-derived string (see
    /// the module doc HARD RULE) — never the wrapped error's own text.
    Other { message: String },
}

impl ErrorKind {
    /// True for kinds that mean the pipeline could not run this dictation at
    /// all, as opposed to [`ErrorKind::OllamaUnreachable`] (purely
    /// informational — the AC-4 fallback still pastes) or
    /// [`ErrorKind::HistoryPersistFailed`] (also purely informational —
    /// issue #220's dictation-succeeded-but-history-row-lost case).
    pub fn is_blocking(&self) -> bool {
        !matches!(
            self,
            ErrorKind::OllamaUnreachable | ErrorKind::HistoryPersistFailed
        )
    }

    /// The static, kind-derived message safe to show the user / emit over
    /// IPC (HARD RULE: never derived from the wrapped error's own text).
    pub fn message(&self) -> String {
        match self {
            ErrorKind::ModelMissing => {
                "The speech-to-text model is missing or failed to load.".to_string()
            }
            ErrorKind::OllamaUnreachable => {
                "Local AI cleanup is unreachable; used basic cleanup instead.".to_string()
            }
            ErrorKind::MicPermissionDenied => {
                "Microphone access is unavailable or was denied.".to_string()
            }
            ErrorKind::HistoryPersistFailed => {
                "Couldn't save this dictation to history.".to_string()
            }
            ErrorKind::Other { message } => message.clone(),
        }
    }

    /// The `kind` discriminant string used in the emitted event payload
    /// (`PipelineErrorEvent::kind`).
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::ModelMissing => "ModelMissing",
            ErrorKind::OllamaUnreachable => "OllamaUnreachable",
            ErrorKind::MicPermissionDenied => "MicPermissionDenied",
            ErrorKind::HistoryPersistFailed => "HistoryPersistFailed",
            ErrorKind::Other { .. } => "Other",
        }
    }
}

/// The wire payload for the `pipeline-error` Tauri event (`lib.rs`'s emit
/// sites): `{ kind: string, message: string }`. Built only via
/// [`ErrorKind::as_str`]/[`ErrorKind::message`], so it inherits their
/// static/kind-derived guarantee (HARD RULE, module doc).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PipelineErrorEvent {
    pub kind: String,
    pub message: String,
}

impl From<&ErrorKind> for PipelineErrorEvent {
    fn from(kind: &ErrorKind) -> Self {
        Self {
            kind: kind.as_str().to_string(),
            message: kind.message(),
        }
    }
}

/// Maps a [`crate::pipeline::PipelineError`] (transcription/output
/// failures — never the AC-4 cleanup fallback, which isn't an error) to its
/// [`ErrorKind`]. Never reads the wrapped error's own `Display`/text into the
/// result (HARD RULE, module doc) — only ever branches on which variant
/// fired.
pub fn error_kind_for_pipeline_error(err: &PipelineError) -> ErrorKind {
    match err {
        PipelineError::Stt(SttError::ModelLoad(_)) => ErrorKind::ModelMissing,
        PipelineError::Stt(SttError::Transcription(_)) => ErrorKind::Other {
            message: "Transcription failed.".to_string(),
        },
        PipelineError::Output(_) => ErrorKind::Other {
            message: "Failed to deliver the transcribed text.".to_string(),
        },
    }
}

/// Maps a [`crate::audio::CaptureError`] (capture-start failure) to its
/// [`ErrorKind`]. `NoInputDevice` is treated as mic-permission-denied: on
/// macOS, a TCC mic-permission denial makes device enumeration itself come
/// back empty rather than surfacing a distinct permission error from `cpal`
/// — so "no input device found" at capture-start is, in practice, almost
/// always a permission problem rather than genuinely no hardware.
pub fn error_kind_for_capture_error(err: &CaptureError) -> ErrorKind {
    match err {
        CaptureError::NoInputDevice => ErrorKind::MicPermissionDenied,
        CaptureError::Cpal(_) => ErrorKind::Other {
            message: "The microphone could not be opened.".to_string(),
        },
        CaptureError::Timeout => ErrorKind::Other {
            message: "The microphone did not start in time.".to_string(),
        },
    }
}

/// Maps the `lib.rs::build_stt` model-missing `String` error surface (which
/// predates a real `SttError` there — see that function's module doc) to its
/// [`ErrorKind`]. Always [`ErrorKind::ModelMissing`]: every `build_stt` `Err`
/// means no STT engine is available to run this dictation. The message
/// itself is intentionally ignored (HARD RULE).
pub fn error_kind_for_build_stt_failure(_msg: &str) -> ErrorKind {
    ErrorKind::ModelMissing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_load_stt_errors_map_to_model_missing() {
        let err = PipelineError::Stt(SttError::ModelLoad("missing file".to_string()));
        assert_eq!(error_kind_for_pipeline_error(&err), ErrorKind::ModelMissing);
    }

    #[test]
    fn transcription_stt_errors_map_to_other_with_a_static_message() {
        let err = PipelineError::Stt(SttError::Transcription("engine choked".to_string()));
        let kind = error_kind_for_pipeline_error(&err);
        match kind {
            ErrorKind::Other { message } => assert!(!message.is_empty()),
            other => panic!("expected ErrorKind::Other, got {other:?}"),
        }
    }

    #[test]
    fn model_missing_and_mic_permission_denied_are_blocking() {
        assert!(ErrorKind::ModelMissing.is_blocking());
        assert!(ErrorKind::MicPermissionDenied.is_blocking());
        assert!(ErrorKind::Other {
            message: "x".to_string()
        }
        .is_blocking());
    }

    #[test]
    fn ollama_unreachable_is_informational_not_blocking() {
        assert!(!ErrorKind::OllamaUnreachable.is_blocking());
    }

    // -------------------------------------------------------------
    // Issue #220 (Sentinel SNTL-20260715-bla-PR218-cc04f8b 🟡): a completed
    // dictation's history-row persist failure is informational — the
    // dictation itself already succeeded (pasted/written) by the time
    // `Store::insert_history` runs, so this is a heads-up about a
    // secondary, non-fatal loss (the history row), not a failure of the
    // pipeline run.
    // -------------------------------------------------------------

    #[test]
    fn history_persist_failed_is_informational_not_blocking() {
        assert!(!ErrorKind::HistoryPersistFailed.is_blocking());
    }

    #[test]
    fn history_persist_failed_carries_a_static_kind_and_message() {
        let event = PipelineErrorEvent::from(&ErrorKind::HistoryPersistFailed);
        assert_eq!(event.kind, "HistoryPersistFailed");
        assert!(!event.message.is_empty());
    }

    #[test]
    fn capture_error_no_input_device_maps_to_mic_permission_denied() {
        assert_eq!(
            error_kind_for_capture_error(&CaptureError::NoInputDevice),
            ErrorKind::MicPermissionDenied
        );
    }

    #[test]
    fn capture_error_timeout_maps_to_other() {
        assert!(matches!(
            error_kind_for_capture_error(&CaptureError::Timeout),
            ErrorKind::Other { .. }
        ));
    }

    #[test]
    fn build_stt_failure_always_maps_to_model_missing() {
        assert_eq!(
            error_kind_for_build_stt_failure("whatever the message"),
            ErrorKind::ModelMissing
        );
        assert_eq!(
            error_kind_for_build_stt_failure(""),
            ErrorKind::ModelMissing
        );
    }

    #[test]
    fn pipeline_error_event_carries_the_kind_discriminant_and_message() {
        let event = PipelineErrorEvent::from(&ErrorKind::ModelMissing);
        assert_eq!(event.kind, "ModelMissing");
        assert!(!event.message.is_empty());

        let event = PipelineErrorEvent::from(&ErrorKind::OllamaUnreachable);
        assert_eq!(event.kind, "OllamaUnreachable");

        let event = PipelineErrorEvent::from(&ErrorKind::MicPermissionDenied);
        assert_eq!(event.kind, "MicPermissionDenied");

        let event = PipelineErrorEvent::from(&ErrorKind::Other {
            message: "custom".to_string(),
        });
        assert_eq!(event.kind, "Other");
        assert_eq!(event.message, "custom");
    }

    #[test]
    fn hard_rule_mapped_payload_never_contains_the_wrapped_errors_own_text() {
        // Feed a fake error wrapping a fixture "transcript" string and
        // confirm the serialized event payload never contains it — the
        // mapping must derive `message` purely from the ErrorKind variant,
        // never from the source error's own Display/text.
        let fixture_transcript = "the quick brown fox jumps over the lazy dog, this is what I said";
        let err = PipelineError::Stt(SttError::Transcription(fixture_transcript.to_string()));

        let kind = error_kind_for_pipeline_error(&err);
        let event = PipelineErrorEvent::from(&kind);
        let serialized = serde_json::to_string(&event).expect("event must serialize");

        assert!(
            !serialized.contains(fixture_transcript),
            "pipeline-error payload leaked the wrapped error's text: {serialized}"
        );
        assert!(!event.message.contains(fixture_transcript));
    }
}
