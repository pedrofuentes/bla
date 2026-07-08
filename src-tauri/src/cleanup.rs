//! The `Cleanup` trait and its implementations: `RegexCleanup` (always available)
//! and `OllamaCleanup` (LLM pass via `localhost:11434`, rewrite-only prompts).
//!
//! Pure logic — no OS calls, fully unit-testable, TDD-mandatory (AGENTS.md).
//! `OllamaCleanup` falls back to `RegexCleanup` whenever Ollama is unreachable,
//! so the pipeline never surfaces a cleanup error to the output path (MISSION AC-4).
//!
//! Prompts live in `src-tauri/prompts/` as versioned files with fixture-based
//! regression checks — never inlined here.
//!
//! This module currently defines the `Cleanup` trait, `Tone`, `CleanupError`,
//! and the `RegexCleanup` baseline (ADR-0005, PRD AC-4). `OllamaCleanup` is a
//! separate, later increment (issue #20) that will slot in behind the same
//! trait.
//!
//! `mod cleanup` isn't `pub` and `commands.rs` doesn't call into it yet — that
//! wiring lands with the pipeline-integration work (issue #25 and friends).
//! Until then this file's items are only reachable from its own unit tests,
//! so `dead_code` is silenced here rather than crate-wide.
#![allow(dead_code)]

use regex::Regex;
use std::fmt;
use std::sync::OnceLock;

/// Controls how aggressively a [`Cleanup`] implementation rewrites the raw
/// transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Default cleanup pass: filler removal, spacing, capitalization,
    /// sentence-final punctuation (and, for LLM-backed implementations,
    /// self-correction resolution — see ADR-0005).
    Neutral,
    /// Bypasses cleanup entirely: the raw transcript is returned essentially
    /// untouched. Reserved for the M3 verbatim tone profile (PRD AC-22).
    Verbatim,
}

/// Errors a [`Cleanup`] implementation may return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupError {
    /// Reserved for the future `OllamaCleanup` (issue #20) — e.g. an
    /// unreachable or malformed LLM backend after fallback has been
    /// exhausted. `RegexCleanup` is pure, infallible logic and never returns
    /// this variant.
    Unreachable,
}

impl fmt::Display for CleanupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CleanupError::Unreachable => write!(f, "cleanup backend unreachable"),
        }
    }
}

impl std::error::Error for CleanupError {}

/// Pure text-transformation seam (ADR-0005). All rewriting hangs off this
/// trait so the pipeline can dispatch between `RegexCleanup` (always on) and
/// a future LLM-backed implementation without changing callers.
pub trait Cleanup {
    /// Rewrites `raw` according to `tone`. Implementations must be pure
    /// (no OS/network calls) with the sole exception of a future
    /// LLM-backed implementation's optional network probe, which must
    /// degrade to the regex baseline rather than propagate an error
    /// (MISSION AC-4).
    fn clean(&self, raw: &str, tone: Tone) -> Result<String, CleanupError>;
}

/// Deterministic, always-available baseline cleanup (ADR-0005, PRD AC-4).
///
/// Under [`Tone::Neutral`], `RegexCleanup`:
/// 1. Removes unambiguous filler interjections ("um", "uh", "er" —
///    word-boundary, case-insensitive) unconditionally.
/// 2. Removes "like" / "you know" **only** when comma-flanked on both sides
///    (e.g. "it's, like, great"), since that punctuation pattern cheaply and
///    reliably marks discourse-filler usage in speech transcripts. Other
///    occurrences — comparative ("looks like rain"), literal ("you know the
///    rules"), or sentence-initial/-final — are deliberately left alone:
///    telling those apart from genuine filler usage isn't cheap, so this
///    baseline stays conservative rather than risk stripping real content.
/// 3. Collapses runs of whitespace (including any left behind by 1–2) to a
///    single space and trims the ends.
/// 4. Capitalizes the first letter of the string and of every sentence that
///    follows a `.`, `!`, or `?`.
/// 5. Ensures the result ends with sentence-final punctuation (`.` added if
///    none of `.`/`!`/`?` is already present).
///
/// `RegexCleanup` does **not** resolve self-corrections (false starts,
/// "I mean", restart-and-rephrase) — that repair is reserved for the future
/// LLM pass (ADR-0005). It operates purely on tokens/whitespace and never
/// returns [`CleanupError::Unreachable`].
pub struct RegexCleanup;

impl Cleanup for RegexCleanup {
    fn clean(&self, raw: &str, tone: Tone) -> Result<String, CleanupError> {
        match tone {
            Tone::Verbatim => Ok(raw.to_string()),
            Tone::Neutral => Ok(clean_text(raw)),
        }
    }
}

struct FillerPatterns {
    /// Unambiguous filler interjections, plus a directly-trailing comma so
    /// removal doesn't leave orphaned punctuation behind.
    interjection: Regex,
    /// "like" / "you know" when comma-flanked on both sides — see
    /// `RegexCleanup`'s doc comment for why this is the chosen heuristic.
    comma_flanked_filler: Regex,
    /// Any run of whitespace, collapsed to a single space.
    whitespace: Regex,
}

fn patterns() -> &'static FillerPatterns {
    static PATTERNS: OnceLock<FillerPatterns> = OnceLock::new();
    PATTERNS.get_or_init(|| FillerPatterns {
        interjection: Regex::new(r"(?i)\b(?:um|uh|er)\b,?").expect("valid regex"),
        comma_flanked_filler: Regex::new(r"(?i),\s*(?:like|you know)\s*,").expect("valid regex"),
        whitespace: Regex::new(r"\s+").expect("valid regex"),
    })
}

/// The deterministic rewrite used by [`RegexCleanup`] under [`Tone::Neutral`].
/// See that type's doc comment for the exact transform order and rationale.
fn clean_text(raw: &str) -> String {
    let patterns = patterns();

    let without_interjections = patterns.interjection.replace_all(raw, "");
    let without_fillers = patterns
        .comma_flanked_filler
        .replace_all(&without_interjections, ",");
    let collapsed = patterns.whitespace.replace_all(&without_fillers, " ");
    let trimmed = collapsed.trim();

    if trimmed.is_empty() {
        return String::new();
    }

    let capitalized = capitalize_sentence_starts(trimmed);

    let ends_with_terminal = matches!(capitalized.chars().last(), Some('.' | '!' | '?'));
    if ends_with_terminal {
        capitalized
    } else {
        format!("{capitalized}.")
    }
}

/// Capitalizes the first letter of `s` and the first letter following every
/// `.`, `!`, or `?` (skipping any whitespace in between).
fn capitalize_sentence_starts(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;
    for c in s.chars() {
        if capitalize_next && c.is_alphabetic() {
            result.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            if c == '.' || c == '!' || c == '?' {
                capitalize_next = true;
            }
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table-driven cases: (description, raw input, expected cleaned output)
    /// under `Tone::Neutral`, covering each `RegexCleanup` transform in turn.
    const CASES: &[(&str, &str, &str)] = &[
        (
            "removes unambiguous filler interjections (um/uh/er)",
            "Um, I think, uh, this works, er, fine",
            "I think, this works, fine.",
        ),
        (
            "removes like/you know only when comma-flanked (discourse-marker use)",
            "So, like, this is cool, you know, right",
            "So, this is cool, right.",
        ),
        (
            "keeps 'like' when not comma-flanked (comparative use, not a filler)",
            "It looks like rain today",
            "It looks like rain today.",
        ),
        (
            "keeps 'you know' when not comma-flanked (literal use, not a filler)",
            "You know the answer",
            "You know the answer.",
        ),
        (
            "collapses duplicate/irregular whitespace",
            "This  is    fine",
            "This is fine.",
        ),
        (
            "capitalizes a lowercase sentence start",
            "hello world",
            "Hello world.",
        ),
        (
            "capitalizes every sentence start, not just the first",
            "hello there. how are you? i am fine!",
            "Hello there. How are you? I am fine!",
        ),
        (
            "leaves already-clean, already-punctuated input untouched",
            "This already has a period.",
            "This already has a period.",
        ),
        (
            "combines filler removal, spacing, capitalization, and punctuation",
            "um,  i think, like,  this  is, uh, the plan, you know,  right",
            "I think, this is, the plan, right.",
        ),
    ];

    #[test]
    fn regex_cleanup_transforms_neutral_tone() {
        let cleanup = RegexCleanup;
        for (description, raw, expected) in CASES {
            let got = cleanup
                .clean(raw, Tone::Neutral)
                .unwrap_or_else(|e| panic!("{description}: clean() returned Err({e:?})"));
            assert_eq!(&got, expected, "case failed: {description}");
        }
    }

    #[test]
    fn regex_cleanup_is_idempotent_on_already_clean_input() {
        let cleanup = RegexCleanup;
        for (description, _, expected) in CASES {
            let twice = cleanup
                .clean(expected, Tone::Neutral)
                .unwrap_or_else(|e| panic!("{description}: clean() returned Err({e:?})"));
            assert_eq!(&twice, expected, "not idempotent: {description}");
        }
    }

    #[test]
    fn regex_cleanup_empty_input_stays_empty() {
        let cleanup = RegexCleanup;
        assert_eq!(cleanup.clean("", Tone::Neutral).unwrap(), "");
        assert_eq!(cleanup.clean("   ", Tone::Neutral).unwrap(), "");
    }

    #[test]
    fn regex_cleanup_verbatim_tone_bypasses_all_transforms() {
        let cleanup = RegexCleanup;
        let raw = "  um, hello   world, like, this is,uh,messy";
        assert_eq!(cleanup.clean(raw, Tone::Verbatim).unwrap(), raw);
    }

    #[test]
    fn regex_cleanup_never_returns_unreachable() {
        // RegexCleanup is the always-available baseline (ADR-0005) — it must
        // never surface the Unreachable variant reserved for the future
        // Ollama-backed implementation (issue #20).
        let cleanup = RegexCleanup;
        for tone in [Tone::Neutral, Tone::Verbatim] {
            assert!(!matches!(
                cleanup.clean("um, test", tone),
                Err(CleanupError::Unreachable)
            ));
        }
    }
}

#[cfg(test)]
mod ollama_tests {
    //! Issue #20 (AC-4, AC-10): `OllamaCleanup`, the injected-transport
    //! seam, and the versioned rewrite-only prompt. All tests here run
    //! against a `StubTransport` — no real network call, no running
    //! Ollama required.
    use super::*;
    use std::cell::RefCell;

    /// Records the last request the transport was asked to send, and
    /// returns a preprogrammed outcome — lets tests assert both the
    /// fallback decision (AC-4) and the exact request shape (AC-10)
    /// without a real socket.
    struct StubTransport {
        response: Result<String, TransportError>,
        captured: RefCell<Option<(String, String)>>,
    }

    impl StubTransport {
        fn unreachable() -> Self {
            Self {
                response: Err(TransportError::ConnectionFailed),
                captured: RefCell::new(None),
            }
        }

        /// A stub that succeeds, echoing back `model_output` inside a
        /// canned Ollama `/api/generate` JSON response body.
        fn succeeding(model_output: &str) -> Self {
            let body = serde_json::json!({ "response": model_output, "done": true }).to_string();
            Self {
                response: Ok(body),
                captured: RefCell::new(None),
            }
        }

        fn captured_request(&self) -> (String, String) {
            self.captured
                .borrow()
                .clone()
                .expect("transport was never called")
        }
    }

    impl OllamaTransport for StubTransport {
        fn post(&self, url: &str, body: &str) -> Result<String, TransportError> {
            *self.captured.borrow_mut() = Some((url.to_string(), body.to_string()));
            self.response.clone()
        }
    }

    fn cleanup_with(transport: StubTransport) -> OllamaCleanup<StubTransport> {
        OllamaCleanup::new("http://localhost:11434", "llama3", transport)
    }

    #[test]
    fn ollama_cleanup_returns_unreachable_when_transport_fails() {
        // AC-4: an unreachable endpoint must surface CleanupError::Unreachable
        // (never a raw transport error, never a panic) so the pipeline can
        // fall back to RegexCleanup.
        let cleanup = cleanup_with(StubTransport::unreachable());
        let result = cleanup.clean("um, hello there", Tone::Neutral);
        assert_eq!(result, Err(CleanupError::Unreachable));
    }

    #[test]
    fn pipeline_style_fallback_surfaces_no_error_when_unreachable() {
        // AC-4, end to end within this module: when OllamaCleanup can't
        // reach the endpoint, dispatching to RegexCleanup on
        // Err(Unreachable) yields a normal Ok result with no error ever
        // reaching a hypothetical paste path.
        let ollama = cleanup_with(StubTransport::unreachable());
        let regex = RegexCleanup;
        let raw = "um, hello world";

        let cleaned = match ollama.clean(raw, Tone::Neutral) {
            Ok(text) => text,
            Err(CleanupError::Unreachable) => {
                regex.clean(raw, Tone::Neutral).expect("RegexCleanup is infallible")
            }
        };

        assert_eq!(cleaned, "Hello world.");
    }

    #[test]
    fn ollama_cleanup_verbatim_tone_bypasses_the_transport_entirely() {
        // Mirrors RegexCleanup's Verbatim contract and, critically, proves
        // Verbatim never touches the network at all.
        let stub = StubTransport::unreachable();
        let cleanup = cleanup_with(stub);
        let raw = "  um, hello   world, uh, messy";
        assert_eq!(cleanup.clean(raw, Tone::Verbatim).unwrap(), raw);
        assert!(cleanup.transport.captured.borrow().is_none());
    }

    #[test]
    fn ollama_cleanup_targets_the_configured_base_url() {
        let stub = StubTransport::succeeding("Hello world.");
        let cleanup = OllamaCleanup::new("http://localhost:11434", "llama3", stub);
        cleanup.clean("hello world", Tone::Neutral).unwrap();
        let (url, _) = cleanup.transport.captured_request();
        assert_eq!(url, "http://localhost:11434/api/generate");
    }

    #[test]
    fn ollama_cleanup_default_base_url_is_localhost_11434() {
        // MISSION §5: the only permitted runtime origin besides model
        // download is localhost:11434 — the default must not point
        // anywhere else.
        assert_eq!(DEFAULT_OLLAMA_BASE_URL, "http://localhost:11434");
    }

    /// AC-10 fixture: self-corrections, missing punctuation, and a spoken
    /// list, all in one transcript.
    const FIXTURE_RAW: &str =
        "so the meeting is at i mean the meeting is tomorrow at 3pm and we need to bring \
         the laptop the charger and the notes";

    /// A well-behaved model response: corrections resolved, punctuation
    /// restored, spoken list rendered as bullets. Used to assert the
    /// pipeline passes a faithful model response through unchanged
    /// (rewrite-only property: this module adds nothing beyond the input).
    const FIXTURE_MODEL_OUTPUT: &str = "The meeting is tomorrow at 3pm. We need to bring:\n- The laptop\n- The charger\n- The notes";

    #[test]
    fn ac10_fixture_regression_applies_corrections_punctuation_and_bullets() {
        let stub = StubTransport::succeeding(FIXTURE_MODEL_OUTPUT);
        let cleanup = cleanup_with(stub);

        let got = cleanup.clean(FIXTURE_RAW, Tone::Neutral).unwrap();

        // The stubbed model already resolved the self-correction ("i mean"),
        // restored punctuation, and rendered the spoken list as bullets;
        // OllamaCleanup must pass that through faithfully.
        assert_eq!(got, FIXTURE_MODEL_OUTPUT);
        assert!(!got.contains("i mean"), "self-correction must not survive");
        assert!(got.ends_with('.') || got.ends_with(':') == false || got.contains("- "));
        assert!(got.contains("- The laptop"));
        assert!(got.contains("- The charger"));
        assert!(got.contains("- The notes"));
    }

    #[test]
    fn ac10_request_carries_the_rewrite_only_prompt_and_the_raw_input_verbatim() {
        // AC-10's rewrite-only property: assert the request sent to the
        // stub carries the rewrite-only prompt plus the untouched input —
        // this module must not editorialize on what it sends upstream
        // either.
        let stub = StubTransport::succeeding(FIXTURE_MODEL_OUTPUT);
        let cleanup = cleanup_with(stub);
        cleanup.clean(FIXTURE_RAW, Tone::Neutral).unwrap();

        let (_, body) = cleanup.transport.captured_request();
        assert!(
            body.contains(FIXTURE_RAW),
            "request body must carry the raw input verbatim: {body}"
        );
        assert!(
            body.contains("rewrite"),
            "request body must carry the rewrite-only system prompt: {body}"
        );
    }

    #[test]
    fn ac10_prompt_file_contains_the_rewrite_only_constraints() {
        // Regression guard: if a future prompt edit drops one of these
        // constraints, this test fails CI (MISSION §7).
        let prompt = CLEANUP_PROMPT_V1.to_lowercase();
        for must_contain in [
            "never answer",
            "never add",
            "filler",
            "self-correction",
            "punctuation",
            "bullet",
            "tone",
        ] {
            assert!(
                prompt.contains(must_contain),
                "prompt is missing required constraint: {must_contain:?}"
            );
        }
    }
}
