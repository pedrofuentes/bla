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
        assert!(looks_like_preamble(
            "\"Sure, here is the rewritten text.\""
        ));
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
