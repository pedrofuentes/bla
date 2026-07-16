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
//!
//! ## Privacy invariant (AC-43, MISSION §5/§7)
//!
//! [`ActiveAppName`] is a single-field newtype wrapping a `String` — the
//! app's NAME only. It structurally cannot carry a window title, bounds, a
//! process id, or any other screen-content-adjacent field: there is no
//! second field for such data to live in, so widening this seam to expose
//! more than a name is a visible, reviewable change to this exact type
//! definition, not a quiet field addition elsewhere. This matters because
//! reading a window's TITLE via `active-win-pos-rs` on macOS requires the
//! Screen Recording permission (a title can reveal on-screen document/page
//! content — the very thing this app promises never leaves the device,
//! MISSION §5); the frontmost app's NAME comes from a lighter-weight OS API
//! (NSWorkspace on macOS) and needs no such prompt (#160's plan). This
//! module must never request Screen Recording, and never logs or persists a
//! window title anywhere — only the app name/identifier needed for
//! matching, exactly like `store::HistoryRow.app_name` and
//! `store::ToneRule.app_pattern` already do.

use crate::cleanup::Tone;
use crate::store::{ToneProfile, ToneRule};

/// The focused application's name — the ONLY OS-context data this seam
/// exposes (AC-43). See this module's doc comment for the full privacy
/// rationale. A bare tuple struct around one `String` rather than a
/// multi-field struct: there is nowhere for a title/bounds/pid field to be
/// added without changing this definition itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveAppName(pub String);

/// Injected active-app detection seam (mirrors `cleanup::OllamaTransport`'s
/// role for `OllamaCleanup`): production wiring uses
/// [`RealActiveAppSource`]; tests use a fake that never touches a real
/// window manager.
pub(crate) trait ActiveAppSource {
    /// The currently focused application's name, or `None` when detection
    /// fails (no active window, permission denied, unsupported platform).
    /// Callers must degrade to [`Tone::Neutral`] on `None` rather than
    /// surface an error to the paste path — see [`resolve_tone_for_app`].
    fn current(&self) -> Option<ActiveAppName>;
}

/// The real, `active-win-pos-rs`-backed [`ActiveAppSource`] — OS glue only,
/// no decision-making. Reads exactly one field off the crate's
/// `ActiveWindow` (`app_name`) and discards everything else (`title`,
/// `process_path`, `window_id`, `process_id`, `position`) at this single
/// call site, so a window title can never propagate past this line (AC-43).
/// Any failure (no active window, permission denied) collapses to `None`
/// silently — never logged, never panics — matching the privacy invariant
/// and the "detection failure degrades to Neutral" contract.
struct RealActiveAppSource;

impl ActiveAppSource for RealActiveAppSource {
    fn current(&self) -> Option<ActiveAppName> {
        active_win_pos_rs::get_active_window()
            .ok()
            .map(|window| ActiveAppName(window.app_name))
    }
}

/// Production entry point: detects the currently focused application's name
/// via [`RealActiveAppSource`]. Called from `lib.rs`'s `StartRecording`
/// handling — "matched at hotkey-press time" per issue #202's plan, i.e.
/// before capture begins, not when it ends (the user may have already
/// switched focus — e.g. to the recording pill itself — by then).
pub(crate) fn detect_active_app_name() -> Option<ActiveAppName> {
    RealActiveAppSource.current()
}

/// Maps a stored [`ToneProfile`] (issue #202's persistence-layer type,
/// deliberately narrower than `Tone` — see `ToneProfile`'s own doc comment)
/// to the [`Tone`] the pipeline actually dispatches on. Total and trivial by
/// construction: every `ToneProfile` variant has exactly one corresponding
/// `Tone` variant.
fn tone_profile_to_tone(profile: ToneProfile) -> Tone {
    match profile {
        ToneProfile::Casual => Tone::Casual,
        ToneProfile::Formal => Tone::Formal,
        ToneProfile::Verbatim => Tone::Verbatim,
    }
}

/// Pure glob/case-insensitive match of `pattern` (a `ToneRule::app_pattern`)
/// against `app_name` (an [`ActiveAppName`]'s inner value). Issue #202's
/// chosen rule-matching semantics (documented here since the issue left the
/// exact choice to the implementer):
///
/// - **Case-insensitive**: app names' casing can vary subtly across OS
///   versions/locales ("Mail" vs "mail"), and forcing users to match casing
///   exactly would be a needless footgun for a feature that's supposed to
///   reduce friction.
/// - **Glob wildcards** (`*` matches any run of characters including none,
///   `?` matches exactly one): a plain string is inherently supported too
///   (a pattern with no wildcard characters is just an exact
///   case-insensitive match) — one matcher handles both "Slack" (exact) and
///   "Chrome*" (e.g. matching both "Google Chrome" and "Chrome Canary") or
///   Windows-style "*.exe" patterns without a second code path.
/// - Every other character in `pattern` is matched **literally**, including
///   characters that are regex metacharacters (`.`, `(`, `)`, ...) — an app
///   name like "Mail (Preview)" must be matchable by that exact literal
///   pattern, not have its parentheses misinterpreted as a capture group.
///
/// Implemented by translating `pattern` into an anchored, case-insensitive
/// `regex::Regex` (escaping every literal character, expanding `*`/`?`) —
/// reuses the `regex` crate already in the dependency tree (`cleanup.rs`)
/// rather than hand-rolling a second glob engine.
fn app_pattern_matches(pattern: &str, app_name: &str) -> bool {
    let mut regex_str = String::from("(?i)^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex_str.push_str(".*"),
            '?' => regex_str.push('.'),
            other => regex_str.push_str(&regex::escape(&other.to_string())),
        }
    }
    regex_str.push('$');
    regex::Regex::new(&regex_str)
        .map(|re| re.is_match(app_name))
        .unwrap_or(false)
}

/// The core dispatch decision (issue #202, PRD AC-22, AC-40): given the
/// active app (or `None` on a detection failure) and the current
/// `tone_rules` (in [`crate::store::Store::list_tone_rules`]'s insertion
/// order), resolve the [`Tone`] `PipelineOpts::tone` should carry for this
/// dictation.
///
/// - `app` is `None` (detection failed — no active window, permission
///   denied) → [`Tone::Neutral`]. Never an error (AC-40/binding constraint:
///   detection failure must degrade silently, not surface to the paste
///   path).
/// - No rule's `app_pattern` matches the app name → [`Tone::Neutral`] (the
///   default; `Neutral` is deliberately never a value `tone_rules` itself
///   stores — see `ToneProfile`'s doc comment).
/// - The **first** matching rule (in `rules`' given order — i.e.
///   insertion/id order, since `list_tone_rules` returns rows that way)
///   wins. Precedence policy, documented since multiple overlapping
///   patterns are possible (e.g. an exact "Slack" rule and a glob "S*"
///   rule both matching "Slack"): first-configured rule wins, matching the
///   order the user created rules in, rather than any implicit
///   specificity ranking — simple, predictable, and consistent with how
///   `list_tone_rules` is already ordered for the Tone tab (#203) to
///   display.
pub(crate) fn resolve_tone_for_app(app: Option<&ActiveAppName>, rules: &[ToneRule]) -> Tone {
    let Some(app) = app else {
        return Tone::Neutral;
    };
    rules
        .iter()
        .find(|rule| app_pattern_matches(&rule.app_pattern, &app.0))
        .map(|rule| tone_profile_to_tone(rule.tone))
        .unwrap_or(Tone::Neutral)
}

#[cfg(test)]
mod tests {
    use crate::cleanup::{Cleanup, RegexCleanup, Tone};
    use crate::store::{Store, ToneProfile, ToneRule};

    use super::ActiveAppSource;
    use super::{app_pattern_matches, resolve_tone_for_app, tone_profile_to_tone, ActiveAppName};

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
    fn app_pattern_star_matches_any_surrounding_text() {
        // Anchored (^...$): "Chrome*" matches names STARTING with "Chrome"
        // ("Chrome Canary") but not "Google Chrome" (which doesn't start
        // with "Chrome") — that needs a leading wildcard too ("*Chrome*").
        assert!(app_pattern_matches("Chrome*", "Chrome Canary"));
        assert!(!app_pattern_matches("Chrome*", "Google Chrome"));
        assert!(app_pattern_matches("*Chrome*", "Google Chrome"));
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
            app_pattern: "*Chrome*".to_string(),
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
