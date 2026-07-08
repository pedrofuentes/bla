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
//! Stub — no logic yet; implemented in a later M1 increment.

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
