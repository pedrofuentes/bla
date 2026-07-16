//! Active-app detection (`active-win-pos-rs`) → tone profile selection.
//!
//! Identifies the focused application so `cleanup` can apply a per-app tone
//! (e.g. terse for chat, formal for email) — issue #202, PRD AC-22.
//!
//! OS-integration module (AGENTS.md §OS-integration exemption): the only
//! logic-free part is `RealActiveAppSource`'s single `active_win_pos_rs`
//! call. Everything else here — the glob/case-insensitive pattern matcher
//! and the tone-resolution decision — is pure and TDD-mandatory
//! (AGENTS.md), kept deliberately separate from that one OS call so it's
//! fully unit-testable without a live window manager, per the Windows-CI
//! hard rule (issue #165: no `AppState`/`tauri::Wry` types in `#[cfg(test)]`
//! code) and this issue's own constraint (never a real `active-win-pos-rs`
//! call in tests — see `FakeActiveAppSource` below).

#[cfg(test)]
mod tests {
    use crate::cleanup::{Cleanup, RegexCleanup, Tone};
    use crate::store::{Store, ToneProfile, ToneRule};

    use super::{app_pattern_matches, resolve_tone_for_app, tone_profile_to_tone, ActiveAppName};
    use super::ActiveAppSource;

    // -------------------------------------------------------------
    // AC-43: the active-app detection seam's public type carries only an
    // app-name String.
    // -------------------------------------------------------------

    #[test]
    fn active_app_name_carries_only_an_app_name_string_ac43() {
        // Compile-shape proof: constructing an ActiveAppName from nothing
        // but a bare String is the whole story — there is no second field
        // this literal could also populate (a title, bounds, pid, ...). If
        // a future edit ever widened the type, this literal (and every
        // other call site) would need a conscious, reviewable update.
        let app = ActiveAppName("Notes".to_string());
        assert_eq!(app.0, "Notes");
    }

    /// A controllable [`ActiveAppSource`] test double — never touches a
    /// real window manager (issue #202's explicit constraint: "never a real
    /// active-win-pos-rs call in tests").
    struct FakeActiveAppSource {
        current: Option<ActiveAppName>,
    }

    impl FakeActiveAppSource {
        fn new(name: Option<&str>) -> Self {
            Self {
                current: name.map(|n| ActiveAppName(n.to_string())),
            }
        }
    }

    impl ActiveAppSource for FakeActiveAppSource {
        fn current(&self) -> Option<ActiveAppName> {
            self.current.clone()
        }
    }

    #[test]
    fn fake_active_app_source_reports_the_configured_app_name() {
        let source = FakeActiveAppSource::new(Some("Slack"));
        assert_eq!(source.current(), Some(ActiveAppName("Slack".to_string())));
    }

    #[test]
    fn fake_active_app_source_reports_none_on_a_simulated_detection_failure() {
        let source = FakeActiveAppSource::new(None);
        assert_eq!(source.current(), None);
    }

    // -------------------------------------------------------------
    // app_pattern_matches: glob/case-insensitive semantics.
    // -------------------------------------------------------------

    #[test]
    fn app_pattern_matches_an_exact_pattern_case_insensitively() {
        assert!(app_pattern_matches("Slack", "Slack"));
        assert!(app_pattern_matches("slack", "Slack"));
        assert!(app_pattern_matches("SLACK", "slack"));
    }

    #[test]
    fn app_pattern_does_not_match_an_unrelated_name() {
        assert!(!app_pattern_matches("Slack", "Mail"));
    }

    #[test]
    fn app_pattern_does_not_partial_match_without_a_wildcard() {
        // Anchored: "Mail" must not match "Mail (Preview)" unless the
        // pattern itself uses a wildcard for the extra text.
        assert!(!app_pattern_matches("Mail", "Mail (Preview)"));
    }

    #[test]
    fn app_pattern_star_matches_any_trailing_text() {
        assert!(app_pattern_matches("Chrome*", "Google Chrome"));
        assert!(app_pattern_matches("Chrome*", "Chrome Canary"));
        assert!(app_pattern_matches("*.exe", "notepad.exe"));
        assert!(!app_pattern_matches("Chrome*", "Firefox"));
    }

    #[test]
    fn app_pattern_question_mark_matches_exactly_one_character() {
        assert!(app_pattern_matches("iChat?", "iChat1"));
        assert!(!app_pattern_matches("iChat?", "iChat"));
        assert!(!app_pattern_matches("iChat?", "iChat12"));
    }

    #[test]
    fn app_pattern_treats_regex_metacharacters_as_literal_text() {
        // "Mail (Preview)"'s parentheses must NOT be interpreted as a regex
        // capture group — they must match only that literal text.
        assert!(app_pattern_matches("Mail (Preview)", "Mail (Preview)"));
        assert!(!app_pattern_matches("Mail (Preview)", "Mail Preview"));
        assert!(!app_pattern_matches("Mail (Preview)", "Mail X"));
    }

    // -------------------------------------------------------------
    // resolve_tone_for_app: the core dispatch decision.
    // -------------------------------------------------------------

    #[test]
    fn resolve_tone_for_app_is_neutral_when_detection_failed() {
        assert_eq!(resolve_tone_for_app(None, &[]), Tone::Neutral);
    }

    #[test]
    fn resolve_tone_for_app_is_neutral_when_no_rule_matches() {
        let app = ActiveAppName("Notes".to_string());
        let rules = vec![ToneRule {
            id: 1,
            app_pattern: "Slack".to_string(),
            tone: ToneProfile::Casual,
            created_at_ms: 1_000,
        }];
        assert_eq!(resolve_tone_for_app(Some(&app), &rules), Tone::Neutral);
    }

    #[test]
    fn resolve_tone_for_app_dispatches_the_matching_rules_tone() {
        let app = ActiveAppName("Slack".to_string());
        let rules = vec![ToneRule {
            id: 1,
            app_pattern: "Slack".to_string(),
            tone: ToneProfile::Casual,
            created_at_ms: 1_000,
        }];
        assert_eq!(resolve_tone_for_app(Some(&app), &rules), Tone::Casual);
    }

    #[test]
    fn resolve_tone_for_app_matches_via_a_glob_pattern() {
        let app = ActiveAppName("Google Chrome".to_string());
        let rules = vec![ToneRule {
            id: 1,
            app_pattern: "Chrome*".to_string(),
            tone: ToneProfile::Formal,
            created_at_ms: 1_000,
        }];
        assert_eq!(resolve_tone_for_app(Some(&app), &rules), Tone::Formal);
    }

    #[test]
    fn resolve_tone_for_app_first_matching_rule_in_list_order_wins() {
        let app = ActiveAppName("Slack".to_string());
        let rules = vec![
            ToneRule {
                id: 1,
                app_pattern: "Slack".to_string(),
                tone: ToneProfile::Casual,
                created_at_ms: 1_000,
            },
            ToneRule {
                id: 2,
                app_pattern: "S*".to_string(),
                tone: ToneProfile::Formal,
                created_at_ms: 2_000,
            },
        ];
        assert_eq!(resolve_tone_for_app(Some(&app), &rules), Tone::Casual);
    }

    #[test]
    fn tone_profile_to_tone_maps_every_profile_to_its_corresponding_tone() {
        assert_eq!(tone_profile_to_tone(ToneProfile::Casual), Tone::Casual);
        assert_eq!(tone_profile_to_tone(ToneProfile::Formal), Tone::Formal);
        assert_eq!(tone_profile_to_tone(ToneProfile::Verbatim), Tone::Verbatim);
    }

    // -------------------------------------------------------------
    // AC-40: editing a tone rule changes dispatch on the very next
    // dictation — no restart, no cache to invalidate, since
    // resolve_tone_for_app reads whatever Store::list_tone_rules returns
    // right now. Demonstrated end to end: Store CRUD -> resolve_tone_for_app
    // -> an actual Cleanup trait dispatch that observably differs before and
    // after the edit.
    // -------------------------------------------------------------

    #[test]
    fn editing_a_tone_rule_changes_cleanup_dispatch_on_the_next_dictation_ac40() {
        let store = Store::open_in_memory().unwrap();
        let app = ActiveAppName("TestApp".to_string());
        let raw = "um, hello there, this is a test";

        store
            .upsert_tone_rule("TestApp", ToneProfile::Verbatim, 1_000)
            .unwrap();
        let rules_before = store.list_tone_rules().unwrap();
        let tone_before = resolve_tone_for_app(Some(&app), &rules_before);
        assert_eq!(tone_before, Tone::Verbatim);
        let cleaned_before = RegexCleanup.clean(raw, tone_before).unwrap();
        assert_eq!(
            cleaned_before, raw,
            "Verbatim must dispatch to the bypass path unchanged"
        );

        // The rule edit: same app_pattern, a different tone. AC-41's upsert
        // semantics mean this updates the existing row, not adds a second.
        store
            .upsert_tone_rule("TestApp", ToneProfile::Casual, 2_000)
            .unwrap();
        let rules_after = store.list_tone_rules().unwrap();
        assert_eq!(rules_after.len(), 1, "the edit must not add a second rule");
        let tone_after = resolve_tone_for_app(Some(&app), &rules_after);
        assert_eq!(tone_after, Tone::Casual);
        let cleaned_after = RegexCleanup.clean(raw, tone_after).unwrap();

        assert_ne!(
            cleaned_before, cleaned_after,
            "Cleanup::clean's actual dispatch must observably differ before vs after the edit"
        );
        assert_eq!(cleaned_after, "Hello there, this is a test.");
    }

    #[test]
    fn deleting_a_tone_rule_reverts_dispatch_to_neutral_ac40() {
        let store = Store::open_in_memory().unwrap();
        let app = ActiveAppName("TestApp".to_string());

        let id = store
            .upsert_tone_rule("TestApp", ToneProfile::Formal, 1_000)
            .unwrap();
        let rules = store.list_tone_rules().unwrap();
        assert_eq!(resolve_tone_for_app(Some(&app), &rules), Tone::Formal);

        store.delete_tone_rule(id).unwrap();
        let rules_after_delete = store.list_tone_rules().unwrap();
        assert_eq!(
            resolve_tone_for_app(Some(&app), &rules_after_delete),
            Tone::Neutral
        );
    }
}
