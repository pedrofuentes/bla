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
//!
//! File-mode target (this increment, AC-3/AC-11): `Clock` is an injected
//! date/time value (never the real OS clock) so `expand_template` and
//! `append_entry` are deterministic and unit-testable; a later increment
//! adds the cursor-paste target (issue #21) and the router that dispatches
//! between them.

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

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

        assert_eq!(
            written,
            dir.path().join("vault/daily/2026-07-07.md")
        );
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

        let written = append_entry(&config, "nested dirs entry", clock(2026, 12, 3, 23, 59)).unwrap();

        assert_eq!(written, dir.path().join("2026/12/03.md"));
        let contents = fs::read_to_string(&written).unwrap();
        assert_eq!(contents, "[23:59]nested dirs entry\n");
    }
}
