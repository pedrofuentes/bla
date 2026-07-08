//! Output router: the only path that writes recognized text somewhere.
//!
//! Two targets: clipboard-swap + synthesized paste (`enigo`) into the focused
//! app, or templated append to a Markdown file (`{{date:YYYY-MM-DD}}` path
//! templating, optional timestamps — the Obsidian daily-note flow).
//!
//! OS-integration module (AGENTS.md §OS-integration exemption) for the paste
//! path; path templating itself should stay pure-logic and unit-testable.
//! Never logs or persists raw clipboard contents (MISSION §5).
//!
//! File-mode target (this increment, AC-3/AC-11): `Clock` is an injected
//! date/time value (never the real OS clock) so `expand_template` and
//! `append_entry` are deterministic and unit-testable; a later increment
//! adds the cursor-paste target (issue #21) and the router that dispatches
//! between them.
//!
//! `dead_code` is allowed at module scope: nothing outside this module's own
//! tests calls the file-mode API yet — the output router (issue #21) is the
//! future, non-test consumer that will dispatch cursor-paste vs. file mode
//! and call into `append_entry`. Remove this allow once that wiring lands.
#![allow(dead_code)]

use std::fs;
use std::io::{self, Write as _};
use std::path::PathBuf;

/// A calendar date + wall-clock time, injected wherever templating or
/// timestamping needs "now" — never read from the OS clock inside this
/// module, so callers can pass a fixed value and keep tests deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

/// Configuration for the file-mode output target: a path template
/// (expanded via [`expand_template`]) plus an optional per-entry timestamp
/// prefix template (also expanded via [`expand_template`], then prepended
/// to each appended entry).
#[derive(Debug, Clone)]
pub struct FileConfig {
    pub path_template: String,
    pub timestamp_prefix_template: Option<String>,
}

/// Expand `{{date:...}}` and `{{time:...}}` tokens in `template` against an
/// injected [`Clock`]. The format string inside a `date`/`time` token may
/// combine the tokens `YYYY`, `MM`, `DD` (date) or `HH`, `mm` (time) with
/// arbitrary literal separators (e.g. `{{date:YYYY/MM/DD}}`), and arbitrary
/// literal text may surround any `{{...}}` placeholder. Placeholders whose
/// kind isn't `date` or `time` are left untouched verbatim.
pub fn expand_template(template: &str, clock: Clock) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        match after_open.find("}}") {
            Some(end) => {
                let token = &after_open[..end];
                out.push_str(&render_token(token, clock));
                rest = &after_open[end + 2..];
            }
            None => {
                // Unterminated placeholder: emit the remainder verbatim.
                out.push_str(&rest[start..]);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn render_token(token: &str, clock: Clock) -> String {
    if let Some(fmt) = token.strip_prefix("date:") {
        render_date_format(fmt, clock)
    } else if let Some(fmt) = token.strip_prefix("time:") {
        render_time_format(fmt, clock)
    } else {
        format!("{{{{{token}}}}}")
    }
}

fn render_date_format(fmt: &str, clock: Clock) -> String {
    fmt.replace("YYYY", &format!("{:04}", clock.year))
        .replace("MM", &format!("{:02}", clock.month))
        .replace("DD", &format!("{:02}", clock.day))
}

fn render_time_format(fmt: &str, clock: Clock) -> String {
    fmt.replace("HH", &format!("{:02}", clock.hour))
        .replace("mm", &format!("{:02}", clock.minute))
}

/// Transcript text in transit through the clipboard-swap paste path
/// (ADR-0003, PRD AC-9).
///
/// Deliberately implements **neither** [`std::fmt::Debug`],
/// [`std::fmt::Display`], nor `serde::Serialize`: that makes it impossible
/// for clipboard/transcript contents to flow into a log macro, string
/// formatting, or a serializer by construction, rather than relying on
/// reviewer vigilance. `clipboard_payload_trait_assertions` below locks this
/// in at compile time — adding any of those trait impls back fails
/// `cargo test`.
///
/// The only way to get the text back out is [`ClipboardPayload::into_inner`],
/// which the paste/restore glue below consumes exactly once.
pub struct ClipboardPayload(String);

impl ClipboardPayload {
    /// Wrap transcript (or saved-clipboard) text for transit through the
    /// paste path.
    pub fn new(text: String) -> Self {
        Self(text)
    }

    /// Consume the payload, releasing the wrapped text. Used only by the
    /// clipboard-swap paste glue — never pass the result to a logger.
    pub fn into_inner(self) -> String {
        self.0
    }
}

/// Given the transcript text we wrote to the clipboard (`set_to`) and what
/// the clipboard actually holds after the synthetic paste + restore delay
/// (`observed`), decide whether it's safe to restore the pre-dictation
/// clipboard contents (ADR-0003, PRD AC-9).
///
/// Restoring is safe exactly when nothing else touched the clipboard while
/// we owned it — i.e. `observed` still equals what we set. If some other
/// actor wrote to the clipboard in the meantime, `observed` differs from
/// `set_to`, and clobbering that newer value with the old, pre-dictation
/// contents would lose the user's data — so the restore is skipped.
pub fn should_restore_clipboard(set_to: &str, observed: &str) -> bool {
    observed == set_to
}

/// Append `entry` to the file-mode target described by `config`, resolving
/// the templated path against `clock`. Creates any missing intermediate
/// directories and the file itself if absent (AC-3), and prepends the
/// expanded timestamp prefix (if configured) to the entry (AC-11). Returns
/// the resolved path that was written to.
pub fn append_entry(config: &FileConfig, entry: &str, clock: Clock) -> io::Result<PathBuf> {
    let path = PathBuf::from(expand_template(&config.path_template, clock));

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let prefix = config
        .timestamp_prefix_template
        .as_deref()
        .map(|t| expand_template(t, clock))
        .unwrap_or_default();

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{prefix}{entry}")?;

    Ok(path)
}

/// Compile-time proof that [`ClipboardPayload`] can never flow into a log
/// macro, string formatting, or a serializer (ADR-0003, PRD AC-9): if
/// `Debug`, `Display`, or `serde::Serialize` were ever added back to it,
/// this assertion fails to compile and `cargo test` fails with it.
#[cfg(test)]
mod clipboard_payload_trait_assertions {
    use super::ClipboardPayload;
    use static_assertions::assert_not_impl_any;

    assert_not_impl_any!(ClipboardPayload: std::fmt::Debug, std::fmt::Display, serde::Serialize);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn confine_accepts_a_plain_relative_path() {
        let base = PathBuf::from("/vault");
        let out = confine_relative_path(&base, "2026-07-07.md").unwrap();
        assert_eq!(out, PathBuf::from("/vault/2026-07-07.md"));
    }

    #[test]
    fn confine_accepts_a_nested_relative_path() {
        let base = PathBuf::from("/vault");
        let out = confine_relative_path(&base, "daily/2026/07-07.md").unwrap();
        assert_eq!(out, PathBuf::from("/vault/daily/2026/07-07.md"));
    }

    #[test]
    fn confine_accepts_a_backtrack_that_stays_within_the_base_dir() {
        let base = PathBuf::from("/vault");
        // "daily/../notes.md" nets to "notes.md" — never dips below base.
        let out = confine_relative_path(&base, "daily/../notes.md").unwrap();
        assert_eq!(out, PathBuf::from("/vault/daily/../notes.md"));
    }

    #[test]
    fn confine_rejects_an_absolute_path() {
        let base = PathBuf::from("/vault");
        let err = confine_relative_path(&base, "/etc/passwd").unwrap_err();
        assert_eq!(err, PathConfinementError::AbsolutePath);
    }

    #[test]
    fn confine_rejects_traversal_that_escapes_the_base_dir() {
        let base = PathBuf::from("/vault");
        let err = confine_relative_path(&base, "../../etc/passwd").unwrap_err();
        assert_eq!(err, PathConfinementError::EscapesBaseDir);
    }

    #[test]
    fn confine_rejects_traversal_that_dips_below_base_even_if_it_would_return() {
        let base = PathBuf::from("/vault");
        // Climbs above the base dir at the second component even though a
        // later "back" component would net out non-negative overall.
        let err = confine_relative_path(&base, "../vault/notes.md").unwrap_err();
        assert_eq!(err, PathConfinementError::EscapesBaseDir);
    }

    #[test]
    fn restores_when_clipboard_still_holds_what_we_set_ac9() {
        // Nobody else touched the clipboard during the restore delay: safe
        // to restore the pre-dictation contents.
        assert!(should_restore_clipboard(
            "transcript we set",
            "transcript we set"
        ));
    }

    #[test]
    fn skips_restore_when_another_actor_changed_the_clipboard_meanwhile() {
        // Some other app wrote to the clipboard after our paste — restoring
        // now would clobber that newer value (ADR-0003 skip-on-change rule).
        assert!(!should_restore_clipboard(
            "transcript we set",
            "someone else's newer clipboard value"
        ));
    }

    #[test]
    fn clipboard_payload_round_trips_its_text_via_the_single_consumption_path() {
        let payload = ClipboardPayload::new("hello from the transcript".to_string());
        assert_eq!(payload.into_inner(), "hello from the transcript");
    }

    fn clock(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> Clock {
        Clock {
            year,
            month,
            day,
            hour,
            minute,
        }
    }

    #[test]
    fn expands_date_token() {
        let out = expand_template("{{date:YYYY-MM-DD}}.md", clock(2026, 7, 7, 9, 5));
        assert_eq!(out, "2026-07-07.md");
    }

    #[test]
    fn expands_date_and_time_tokens_with_literal_text_around_them() {
        let out = expand_template(
            "journal/{{date:YYYY}}/{{date:MM}}-{{date:DD}} notes ({{time:HH:mm}}).md",
            clock(2026, 7, 7, 9, 5),
        );
        assert_eq!(out, "journal/2026/07-07 notes (09:05).md");
    }

    #[test]
    fn expands_alternate_date_separator_variant() {
        let out = expand_template("{{date:YYYY/MM/DD}}.md", clock(2026, 1, 2, 0, 0));
        assert_eq!(out, "2026/01/02.md");
    }

    #[test]
    fn leaves_unknown_tokens_untouched() {
        let out = expand_template("{{unknown:foo}}.md", clock(2026, 7, 7, 9, 5));
        assert_eq!(out, "{{unknown:foo}}.md");
    }

    #[test]
    fn append_entry_creates_missing_dirs_and_file_ac3() {
        let dir = tempdir().unwrap();
        let template = dir
            .path()
            .join("vault/daily/{{date:YYYY-MM-DD}}.md")
            .to_string_lossy()
            .into_owned();
        let config = FileConfig {
            path_template: template,
            timestamp_prefix_template: Some("{{time:HH:mm}} ".to_string()),
        };

        let written = append_entry(&config, "hello world", clock(2026, 7, 7, 9, 5)).unwrap();

        assert_eq!(written, dir.path().join("vault/daily/2026-07-07.md"));
        let contents = fs::read_to_string(&written).unwrap();
        assert_eq!(contents, "09:05 hello world\n");
    }

    #[test]
    fn append_entry_appends_to_existing_file_without_overwriting() {
        let dir = tempdir().unwrap();
        let template = dir
            .path()
            .join("{{date:YYYY-MM-DD}}.md")
            .to_string_lossy()
            .into_owned();
        let config = FileConfig {
            path_template: template,
            timestamp_prefix_template: None,
        };
        let same_day = clock(2026, 7, 7, 9, 5);

        let path1 = append_entry(&config, "first entry", same_day).unwrap();
        let path2 = append_entry(&config, "second entry", same_day).unwrap();

        assert_eq!(path1, path2);
        let contents = fs::read_to_string(&path1).unwrap();
        assert_eq!(contents, "first entry\nsecond entry\n");
    }

    #[test]
    fn append_entry_supports_multiple_template_variants_ac11() {
        let dir = tempdir().unwrap();
        let template = dir
            .path()
            .join("{{date:YYYY}}/{{date:MM}}/{{date:DD}}.md")
            .to_string_lossy()
            .into_owned();
        let config = FileConfig {
            path_template: template,
            timestamp_prefix_template: Some("[{{time:HH:mm}}]".to_string()),
        };

        let written =
            append_entry(&config, "nested dirs entry", clock(2026, 12, 3, 23, 59)).unwrap();

        assert_eq!(written, dir.path().join("2026/12/03.md"));
        let contents = fs::read_to_string(&written).unwrap();
        assert_eq!(contents, "[23:59]nested dirs entry\n");
    }
}
