//! Speech-to-text via `whisper-rs` (whisper.cpp bindings), Metal-accelerated on macOS.
//!
//! Transcribes the audio buffer produced by `audio`. Personal-dictionary terms
//! (from `store`) are passed as Whisper's `initial_prompt` to bias recognition
//! toward the user's vocabulary.
//!
//! Pure-logic-adjacent: the whisper.cpp call is native glue, but pre/post
//! processing (prompt construction, output normalization) should stay unit-testable.

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_terms_yield_empty_prompt() {
        assert_eq!(build_initial_prompt(&[]), "");
    }

    #[test]
    fn single_term_is_rendered_verbatim() {
        assert_eq!(build_initial_prompt(&terms(&["Kubernetes"])), "Kubernetes");
    }

    #[test]
    fn multiple_terms_preserve_input_order() {
        assert_eq!(
            build_initial_prompt(&terms(&["gamma", "alpha", "beta"])),
            "gamma, alpha, beta"
        );
    }

    #[test]
    fn blank_and_whitespace_only_terms_are_dropped() {
        assert_eq!(build_initial_prompt(&terms(&["", "   ", "kubectl"])), "kubectl");
    }

    #[test]
    fn internal_whitespace_and_newlines_collapse_to_single_spaces() {
        assert_eq!(
            build_initial_prompt(&terms(&["foo\n bar\t baz"])),
            "foo bar baz"
        );
    }

    #[test]
    fn commas_in_terms_are_escaped_so_the_join_stays_unambiguous() {
        assert_eq!(build_initial_prompt(&terms(&["Acme, Inc."])), "Acme\\, Inc.");
    }

    #[test]
    fn backslashes_are_escaped_before_commas_are() {
        assert_eq!(build_initial_prompt(&terms(&["C:\\Users"])), "C:\\\\Users");
    }

    #[test]
    fn length_cap_truncates_at_a_term_boundary_without_exceeding_the_cap() {
        let term = "a".repeat(100);
        let terms: Vec<String> = std::iter::repeat(term).take(20).collect();
        let prompt = build_initial_prompt(&terms);
        assert!(prompt.len() <= INITIAL_PROMPT_MAX_CHARS);
        for part in prompt.split(", ") {
            assert_eq!(part.len(), 100);
        }
        assert!(!prompt.is_empty());
    }

    #[test]
    fn transcribe_opts_initial_prompt_delegates_to_build_initial_prompt() {
        let opts = TranscribeOpts {
            dictionary: terms(&["foo", "bar"]),
        };
        assert_eq!(opts.initial_prompt(), build_initial_prompt(&opts.dictionary));
    }

    #[test]
    fn fake_stt_returns_its_canned_transcript_regardless_of_input() {
        let stt = FakeStt::new("hello world");
        let samples = [0.0_f32; 16_000];
        let opts = TranscribeOpts::default();
        assert_eq!(stt.transcribe(&samples, &opts).unwrap(), "hello world");
    }

    #[test]
    fn fake_stt_default_returns_a_nonempty_canned_transcript() {
        let stt = FakeStt::default();
        assert!(!stt
            .transcribe(&[], &TranscribeOpts::default())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn stt_trait_is_object_safe_and_usable_as_a_trait_object() {
        let stt: Box<dyn Stt> = Box::new(FakeStt::new("boxed"));
        assert_eq!(
            stt.transcribe(&[], &TranscribeOpts::default()).unwrap(),
            "boxed"
        );
    }

    #[test]
    fn transcribe_opts_carries_dictionary_terms_through_to_initial_prompt() {
        let opts = TranscribeOpts {
            dictionary: terms(&["Kubernetes", "kubectl"]),
        };
        assert_eq!(opts.initial_prompt(), "Kubernetes, kubectl");
    }
}
