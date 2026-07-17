//! Detection of LLM "preamble" / prompt-echo pollution in model output
//! (issues #282, #283).
//!
//! The hardcoded local model (llama3, 8B) sometimes emits a conversational
//! preamble, a task-narration, or a label instead of returning ONLY the
//! rewritten text — e.g. it narrates its own system prompt ("The user has
//! selected some text (the CONTENT CHANNEL)… My task is to…", #282) or
//! prepends a chatty header ("This is a formal rewrite of your original
//! transcript: …", #283). Nothing downstream strips or prevents that, so a
//! narrated prompt gets pasted over the user's selection (command mode) or a
//! preamble is emitted as the "cleaned" dictation (cleanup/tone mode).
//!
//! [`looks_like_preamble`] is a **conservative, model-independent** detector
//! for that failure mode. It is deliberately biased toward false negatives
//! over false positives: a legitimate rewrite that merely happens to start
//! with "This" (e.g. "This is normal.") must never be flagged — only the
//! specific narration/label/echo shapes are. It is pure logic (no OS/network,
//! fully unit-tested) and never inspects or logs the *meaning* of the text,
//! only its leading shape and a couple of the prompt's own distinctive
//! uppercase labels (MISSION §5: this module logs nothing).

/// The command prompt's own distinctive UPPERCASE channel labels. If either
/// appears anywhere in model *output*, the model is echoing/narrating its
/// system prompt rather than returning a rewrite (the #282 shape) — a
/// faithful rewrite of ordinary selected text does not reproduce these exact
/// uppercase phrases. Matched case-sensitively to stay conservative (a
/// lowercase "content channel" could appear in genuine prose).
const PROMPT_ECHO_MARKERS: &[&str] = &["CONTENT CHANNEL", "INSTRUCTION CHANNEL"];

/// Leading phrases (lowercased) that mark the output as a conversational
/// preamble, a task-narration, a role label, or a "here is the rewrite:"
/// header rather than the rewritten text itself. Matched via `starts_with`
/// on the leading-punctuation-stripped, lowercased output — deliberately
/// specific ("this is a formal rewrite", not bare "this is") so a legitimate
/// rewrite that merely begins with "This"/"Here"/"The" is never flagged
/// (issues #282, #283).
const PREAMBLE_PREFIXES: &[&str] = &[
    // "Sure, here …" / "Of course, here …" / "Certainly, here …"
    "sure, here",
    "sure! here",
    "sure thing",
    "sure, i",
    "of course, here",
    "of course! here",
    "certainly, here",
    "certainly! here",
    // "Here is the rewritten …" style headers (scoped to rewrite/output
    // wording so "Here is where we disagree." is not caught).
    "here is the rewritten",
    "here's the rewritten",
    "here is the rewrite",
    "here's the rewrite",
    "here is a rewritten",
    "here's a rewritten",
    "here is the corrected",
    "here's the corrected",
    "here is your rewritten",
    "here is the formal version",
    "here is the casual version",
    "here is the cleaned",
    // Task-narration (the #282 shape).
    "my task is",
    "your task is",
    "the task is to",
    "the user has selected",
    "the user selected",
    "the user's selected",
    // "This is a … rewrite/version …" labels (the #283 shape) — scoped so
    // "This is a formal document…" / "This is normal." are NOT caught.
    "this is a formal rewrite",
    "this is a casual rewrite",
    "this is a neutral rewrite",
    "this is a formal version",
    "this is a casual version",
    "this is a rewrite of",
    "this is the rewrite of",
    "this is the rewritten",
    "this is a rewritten",
    "this is your rewritten",
    // Refusals / meta.
    "as an ai",
    "i'm just an ai",
    // The model's own turn label.
    "assistant:",
];

/// Whether `output` looks like an LLM preamble / prompt echo rather than the
/// bare rewritten text (issues #282, #283). Conservative by construction —
/// see the module and constant docs for the false-positive discipline.
///
/// Two independent signals:
/// 1. The prompt's own uppercase channel labels appearing anywhere in the
///    output ([`PROMPT_ECHO_MARKERS`]).
/// 2. A [`PREAMBLE_PREFIXES`] phrase at the very start of the output, after
///    stripping leading whitespace and common quote/markdown-emphasis
///    characters a model might wrap a preamble in.
pub fn looks_like_preamble(output: &str) -> bool {
    if PROMPT_ECHO_MARKERS
        .iter()
        .any(|marker| output.contains(marker))
    {
        return true;
    }

    let normalized: String = output
        .trim_start_matches(|c: char| {
            c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '*' | '#' | '“' | '‘')
        })
        .to_lowercase();

    PREAMBLE_PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The literal #282 symptom: llama3 paraphrasing its own system prompt
    /// instead of rewriting the selection. Synthetic reconstruction of the
    /// shape reported in the AC-7 smoke test (ADR-0007: no real user data).
    const NARRATED_PROMPT_282: &str = "The user has selected some text (the CONTENT CHANNEL) and \
         spoke an instruction. My task is to take these two inputs and produce a rewritten \
         version of the content.";

    /// The literal #283 symptom: a conversational label prepended to an
    /// otherwise-valid formal rewrite.
    const LABELED_REWRITE_283: &str =
        "This is a formal rewrite of your original transcript: This is normal.";

    #[test]
    fn flags_the_reported_282_prompt_narration() {
        assert!(looks_like_preamble(NARRATED_PROMPT_282));
    }

    #[test]
    fn flags_the_reported_283_conversational_label() {
        assert!(looks_like_preamble(LABELED_REWRITE_283));
    }

    #[test]
    fn flags_the_prompts_own_uppercase_channel_labels_anywhere() {
        // The command prompt's distinctive uppercase labels appearing in the
        // OUTPUT are an unambiguous prompt-echo signal — a faithful rewrite
        // of ordinary selected text does not reproduce them.
        assert!(looks_like_preamble("Rewritten. (per the CONTENT CHANNEL)"));
        assert!(looks_like_preamble(
            "Following the INSTRUCTION CHANNEL, here goes."
        ));
    }

    #[test]
    fn flags_common_leading_conversational_preambles() {
        for polluted in [
            "Sure! Here is a more concise version: Let's meet next week.",
            "Sure, here is the rewritten text.",
            "Of course, here is the rewrite: Done.",
            "Here is the rewritten text: Done.",
            "Here's the rewritten version: Done.",
            "My task is to make the content more concise.",
            "Your task is to rewrite the selection.",
            "The user selected some text to rewrite.",
            "This is a casual rewrite: hey there.",
            "This is the rewritten text: Done.",
            "As an AI language model, I have rewritten it.",
            "Assistant: here is the result.",
        ] {
            assert!(
                looks_like_preamble(polluted),
                "should flag a leading preamble: {polluted:?}"
            );
        }
    }

    #[test]
    fn tolerates_leading_whitespace_quotes_and_markdown_emphasis() {
        assert!(looks_like_preamble("   My task is to rewrite this."));
        assert!(looks_like_preamble("\"Sure, here is the rewritten text.\""));
        assert!(looks_like_preamble("**Here is the rewritten text:** Done."));
    }

    #[test]
    fn does_not_flag_legitimate_rewrites() {
        // These are exactly the kinds of faithful outputs the detector must
        // let through untouched — including ones that start with "This",
        // "Here", or "The", which a naive prefix check would wrongly catch.
        for clean in [
            "This is normal.",
            "This is a great idea worth pursuing.",
            "This is a formal document describing the tax rules.",
            "Let's meet Wednesday.",
            "The meeting is tomorrow at 3pm.",
            "Here is where we fundamentally disagree.",
            "Please disregard that request; here is a concise rewrite.",
            "We need to bring:\n- The laptop\n- The charger\n- The notes",
            "Dear Sir or Madam, I am writing to follow up on our conversation.",
        ] {
            assert!(
                !looks_like_preamble(clean),
                "must NOT flag a legitimate rewrite: {clean:?}"
            );
        }
    }

    #[test]
    fn empty_and_whitespace_are_not_flagged_as_preamble() {
        // Blank/whitespace-only output is a *separate* failure mode (the
        // existing EmptyResult guard) — the preamble detector must not also
        // claim it, so the two guards stay cleanly distinguishable.
        assert!(!looks_like_preamble(""));
        assert!(!looks_like_preamble("   \n  "));
    }
}
