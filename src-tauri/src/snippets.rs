//! Snippet trigger matching (pure) — issue #260, AC-52, part of #242's M4
//! scope.
//!
//! Resolves whether a transcript contains a configured snippet's trigger
//! phrase and, if so, which stored [`Snippet::body`] it maps to. Mirrors
//! `context.rs`'s `app_pattern_matches` / `resolve_tone_for_app` shape: a
//! pure function over injected data (no OS/network calls), first-match-in
//! -list-order wins. `store::Snippet` (issue #258) is reused directly
//! rather than redefined here, exactly like `context.rs` imports
//! `store::ToneRule` rather than owning its own tone-rule type.
//!
//! No pipeline/IPC wiring here — that's #263. Table-driven unit tests over
//! synthetic fixtures only (ADR-0007); no `AppState`/`tauri::Wry` types
//! (issue #165's Windows-CI hard rule) — this module needs neither.
//!
//! ## Privacy invariant (MISSION §5/§7)
//!
//! `text` (the transcript being matched against), and every [`Snippet`]'s
//! `trigger`/`body`, are user content — nothing in this module
//! `println!`/`log!`s any of them, mirroring [`Snippet`]'s own doc comment
//! on `store.rs`.

#![allow(dead_code)] // Not yet wired to the pipeline/commands layer (#263).

use crate::store::Snippet;

/// Given a transcript `text` and the caller-supplied `snippets` (typically
/// [`crate::store::Store::list_snippets`]'s result, though this function
/// imposes no ordering of its own — see below), returns the stored
/// [`Snippet::body`] of the **first** snippet (in `snippets`' given slice
/// order) whose `trigger` is present in `text`, or `None` if no trigger
/// matches.
///
/// - **First-match-in-list-order wins** (AC-52, mirroring AC-40's
///   `resolve_tone_for_app` precedent in `context.rs`): if two snippets'
///   triggers both appear in `text`, the one appearing earlier in
///   `snippets` wins — simple and predictable, consistent with how
///   `resolve_tone_for_app` already resolves overlapping `ToneRule`
///   matches. This function does not itself decide what "list order"
///   means (oldest-first, newest-first, ...) — that is entirely up to
///   whatever order the caller passes in.
/// - **Case-insensitive** (AC-52): matches `trigger_matches`'s semantics,
///   see its own doc comment.
/// - **Word-boundary-aware** (AC-52's "documented match-boundary
///   decision"): see `trigger_matches`.
pub(crate) fn match_snippet(text: &str, snippets: &[Snippet]) -> Option<String> {
    snippets
        .iter()
        .find(|snippet| trigger_matches(text, &snippet.trigger))
        .map(|snippet| snippet.body.clone())
}

/// Pure case-insensitive, word-boundary-aware match of `trigger` against
/// `text`. Issue #260's match-boundary decision (documented here since the
/// issue left the exact choice to the implementer):
///
/// - **Case-insensitive**: a spoken trigger phrase transcribed by
///   whisper.cpp can surface with arbitrary capitalization (sentence-start
///   capitalization, a proper-noun-like trigger, ...) that has nothing to
///   do with what the user actually typed when configuring the snippet —
///   forcing exact-case matches would make the feature unreliable for the
///   common case. Same rationale as `context.rs`'s `app_pattern_matches`.
/// - **Word-boundary-anchored, not a bare substring match**: a trigger
///   like "sig" must NOT fire inside an unrelated transcribed word like
///   "signature" or "design" — only when it appears as its own word (or
///   phrase, for multi-word triggers) surrounded by non-word characters or
///   the ends of `text`. A bare substring match would make short, common
///   triggers unusably trigger-happy (a real risk for a dictation feature,
///   where "sig" or "addr"-style short triggers are exactly the kind of
///   thing users configure for speed). This reuses the SAME `\b`
///   word-boundary primitive `cleanup.rs`'s `FillerPatterns::interjection`
///   already relies on to avoid matching "um"/"uh"/"er" inside longer
///   words — established precedent in this crate for boundary-anchored,
///   case-insensitive matching against transcript text.
/// - Implemented by escaping `trigger` into a literal (via `regex::escape`,
///   mirroring `app_pattern_matches`'s own escaping of literal pattern
///   text) and wrapping it in `(?i)\b...\b` — reuses the `regex` crate
///   already in the dependency tree rather than hand-rolling a second
///   boundary-scanning matcher.
/// - An empty `trigger` never matches anything (defensive: `\b\b` against
///   an empty pattern would otherwise match at almost every position in
///   `text`, which is never a meaningful "trigger present" answer).
fn trigger_matches(text: &str, trigger: &str) -> bool {
    if trigger.is_empty() {
        return false;
    }
    let pattern = format!(r"(?i)\b{}\b", regex::escape(trigger));
    regex::Regex::new(&pattern)
        .map(|re| re.is_match(text))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{match_snippet, trigger_matches};
    use crate::store::Snippet;

    /// Synthetic fixture constructor (ADR-0007) — every field is
    /// deliberately made-up, never real dictation text.
    fn snippet(id: i64, trigger: &str, body: &str) -> Snippet {
        Snippet {
            id,
            trigger: trigger.to_string(),
            body: body.to_string(),
            created_at_ms: 1_000 + id,
        }
    }

    // -------------------------------------------------------------
    // trigger_matches: case-insensitive, word-boundary-anchored semantics
    // (AC-52's documented match-boundary decision).
    // -------------------------------------------------------------

    #[test]
    fn trigger_matches_an_exact_standalone_word_ac52() {
        assert!(trigger_matches("please add my sig now", "sig"));
    }

    #[test]
    fn trigger_matches_is_case_insensitive_ac52() {
        assert!(trigger_matches("please add my SIG now", "sig"));
        assert!(trigger_matches("please add my sig now", "SIG"));
    }

    #[test]
    fn trigger_does_not_match_inside_a_longer_word_ac52() {
        // The documented match-boundary decision: "sig" must not fire
        // inside "signature".
        assert!(!trigger_matches("please add your signature below", "sig"));
        assert!(!trigger_matches("this needs a redesign", "sig"));
    }

    #[test]
    fn trigger_matches_at_the_very_start_and_end_of_text() {
        assert!(trigger_matches("sig", "sig"));
        assert!(trigger_matches("sig please", "sig"));
        assert!(trigger_matches("please sig", "sig"));
    }

    #[test]
    fn trigger_matches_when_immediately_followed_by_punctuation() {
        // Punctuation is itself a non-word character, so it counts as a
        // valid boundary even with no surrounding whitespace.
        assert!(trigger_matches("add my sig, then send it", "sig"));
        assert!(trigger_matches("is this the sig?", "sig"));
    }

    #[test]
    fn trigger_matches_a_multi_word_phrase() {
        assert!(trigger_matches(
            "please send best regards to the team",
            "best regards"
        ));
    }

    #[test]
    fn trigger_does_not_multi_word_match_across_unrelated_words() {
        assert!(!trigger_matches(
            "please send my best wishes and regards",
            "best regards"
        ));
    }

    #[test]
    fn trigger_does_not_match_an_unrelated_transcript() {
        assert!(!trigger_matches("what time is the meeting", "sig"));
    }

    #[test]
    fn trigger_with_regex_metacharacters_matches_only_the_literal_text() {
        // Mirrors `app_pattern_matches`'s own metacharacter-escaping test:
        // a trigger containing regex-special characters must be treated
        // literally, not interpreted as a pattern.
        assert!(trigger_matches("my email is a.b@example.com", "a.b"));
        assert!(!trigger_matches("my email is axb@example.com", "a.b"));
    }

    #[test]
    fn empty_trigger_never_matches_ac52() {
        assert!(!trigger_matches("anything at all", ""));
    }

    // -------------------------------------------------------------
    // match_snippet: resolves a transcript to a snippet body, first match
    // in list order wins (AC-52, mirroring AC-40's resolve_tone_for_app
    // precedent).
    // -------------------------------------------------------------

    #[test]
    fn match_snippet_returns_none_for_an_empty_snippet_list() {
        assert_eq!(match_snippet("please add my sig now", &[]), None);
    }

    #[test]
    fn match_snippet_returns_none_when_no_trigger_is_present() {
        let snippets = vec![snippet(1, "sig", "Best, Pat")];
        assert_eq!(match_snippet("what time is the meeting", &snippets), None);
    }

    #[test]
    fn match_snippet_returns_the_matching_snippets_body_ac52() {
        let snippets = vec![snippet(1, "sig", "Best regards,\nPat Nguyen")];
        assert_eq!(
            match_snippet("please add my sig now", &snippets),
            Some("Best regards,\nPat Nguyen".to_string())
        );
    }

    #[test]
    fn match_snippet_first_match_in_list_order_wins_ac52() {
        // Both triggers are present in the transcript; the FIRST
        // configured snippet (list order) must win, not the second, even
        // though the second's trigger also appears.
        let snippets = vec![
            snippet(1, "addr", "123 Main St, Springfield"),
            snippet(2, "sig", "Best, Pat"),
        ];
        assert_eq!(
            match_snippet("send my addr and my sig please", &snippets),
            Some("123 Main St, Springfield".to_string())
        );

        // Reversing the list order flips the winner — proves the decision
        // really is list-order-driven, not e.g. "earliest occurrence in
        // text" or "longest trigger".
        let reversed = vec![
            snippet(2, "sig", "Best, Pat"),
            snippet(1, "addr", "123 Main St, Springfield"),
        ];
        assert_eq!(
            match_snippet("send my addr and my sig please", &reversed),
            Some("Best, Pat".to_string())
        );
    }

    #[test]
    fn match_snippet_skips_non_matching_snippets_to_find_a_later_match() {
        let snippets = vec![
            snippet(1, "addr", "123 Main St"),
            snippet(2, "sig", "Best, Pat"),
        ];
        assert_eq!(
            match_snippet("please add my sig now", &snippets),
            Some("Best, Pat".to_string())
        );
    }

    #[test]
    fn match_snippet_does_not_match_a_trigger_embedded_in_a_longer_word_ac52() {
        let snippets = vec![snippet(1, "sig", "Best, Pat")];
        assert_eq!(
            match_snippet("please add your signature below", &snippets),
            None
        );
    }
}
