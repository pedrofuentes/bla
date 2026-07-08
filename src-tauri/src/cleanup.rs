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
