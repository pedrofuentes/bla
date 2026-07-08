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
//! File-mode target (#41, AC-3/AC-11): `Clock` is an injected date/time
//! value (never the real OS clock) so `expand_template` and `append_entry`
//! are deterministic and unit-testable.
//!
//! Cursor-paste target + router (this increment, issue #21, AC-9,
//! ADR-0003): `ClipboardPayload` carries transcript/clipboard text without
//! `Debug`/`Display`/`Serialize` (locked in by a compile-time
//! trait-assertion test) so it can never be logged or persisted by
//! accident. `Clipboard`/`PasteSynthesizer` are thin OS-glue seams (real
//! impls: `SystemClipboard`/`EnigoPaste`, via `arboard`/`enigo`) behind
//! which `should_restore_clipboard` and `paste_via_clipboard_swap` — the
//! actual save/set/paste/restore-or-skip decision logic — stay pure and
//! fakeable in tests. `route`/`OutputMode` dispatch a finished dictation to
//! either target; the file branch additionally confines its resolved path
//! to a configured base directory (`confine_relative_path`), rejecting
//! absolute paths and `..` traversal that would escape it (security AC
//! carried from PR #41's Sentinel review). Symlink-TOCTOU guarding and
//! restrictive file permissions on the confined target remain noted
//! follow-ups, not addressed here.
//!
//! `route`'s first non-test consumer is `pipeline` (issue #25), which calls
//! it from `Pipeline::run`; `commands.rs` doesn't call into either module
//! yet — that wiring is a later step. `dead_code` stays allowed at module
//! scope for any item not yet reached from there.
#![allow(dead_code)]

use std::fs;
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// Default clipboard-swap restore delay (ADR-0003: 150–300 ms; paste
/// consumers read the clipboard asynchronously, so restoring too early
/// truncates the paste). Tuned further during the AC-7 human smoke test.
pub const DEFAULT_RESTORE_DELAY: Duration = Duration::from_millis(200);

/// Thin OS-glue seam for reading/writing the real system clipboard
/// (AGENTS.md OS-integration exemption). Implemented for real via `arboard`
/// in [`SystemClipboard`]; tests inject a fake so
/// [`paste_via_clipboard_swap`]'s restore-decision logic is exercised
/// without a real clipboard.
pub trait Clipboard {
    fn get(&self) -> io::Result<String>;
    fn set(&self, contents: &str) -> io::Result<()>;
}

/// Thin OS-glue seam for synthesizing the paste keystroke (Cmd+V / Ctrl+V).
/// Implemented for real via `enigo` in [`EnigoPaste`].
pub trait PasteSynthesizer {
    fn synthesize_paste(&self) -> io::Result<()>;
}

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

/// Errors from confining a file-mode target path to its configured base
/// directory (security AC carried from PR #41's Sentinel review into issue
/// #21, now reachable via the output router).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathConfinementError {
    /// The expanded path template was absolute, which would ignore
    /// `base_dir` entirely.
    AbsolutePath,
    /// The expanded path template contains a `..` component that climbs
    /// above `base_dir` at some point while walking it.
    EscapesBaseDir,
}

/// Confine an already-expanded (`{{date}}`/`{{time}}` tokens resolved)
/// relative path to `base_dir`, rejecting absolute paths and any `..`
/// traversal that would climb above `base_dir`.
///
/// Purely lexical: no filesystem access and no symlink resolution. (Note:
/// symlink-TOCTOU on the confined target and restrictive file permissions
/// are follow-up items per issue #21's comment — not addressed here.)
pub fn confine_relative_path(
    base_dir: &Path,
    expanded_relative: &str,
) -> Result<PathBuf, PathConfinementError> {
    let candidate = Path::new(expanded_relative);

    let mut depth: i64 = 0;
    for component in candidate.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(PathConfinementError::EscapesBaseDir);
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(PathConfinementError::AbsolutePath);
            }
        }
    }

    Ok(base_dir.join(candidate))
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

/// Clipboard-swap paste (AC-9, ADR-0003): save the current clipboard, write
/// the transcript, synthesize the paste keystroke, wait `restore_delay`,
/// then restore the saved clipboard unless [`should_restore_clipboard`]
/// says something else changed it meanwhile.
///
/// `clipboard`/`paste` are the thin OS-glue seams (real impls:
/// [`SystemClipboard`]/[`EnigoPaste`]); `sleep` is injected too so tests
/// never actually wait — and, in the skip-on-change test, can simulate a
/// concurrent clipboard write during the delay.
pub fn paste_via_clipboard_swap(
    clipboard: &impl Clipboard,
    paste: &impl PasteSynthesizer,
    sleep: impl FnOnce(Duration),
    payload: ClipboardPayload,
    restore_delay: Duration,
) -> io::Result<()> {
    let saved = clipboard.get()?;
    let transcript = payload.into_inner();
    clipboard.set(&transcript)?;
    paste.synthesize_paste()?;
    sleep(restore_delay);
    let observed = clipboard.get()?;
    if should_restore_clipboard(&transcript, &observed) {
        clipboard.set(&saved)?;
    }
    Ok(())
}

/// Real system clipboard via `arboard`. Thin OS glue (AGENTS.md
/// OS-integration exemption) — no decisions here, just reading/writing the
/// platform clipboard; [`should_restore_clipboard`] and
/// [`paste_via_clipboard_swap`] carry the actual logic and stay behind the
/// [`Clipboard`] trait so they're testable without this type.
pub struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn get(&self) -> io::Result<String> {
        arboard::Clipboard::new()
            .and_then(|mut cb| cb.get_text())
            .map_err(|e| io::Error::other(e.to_string()))
    }

    fn set(&self, contents: &str) -> io::Result<()> {
        arboard::Clipboard::new()
            .and_then(|mut cb| cb.set_text(contents.to_string()))
            .map_err(|e| io::Error::other(e.to_string()))
    }
}

/// Real synthetic Cmd+V (macOS) / Ctrl+V (elsewhere) via `enigo`. Thin OS
/// glue (AGENTS.md OS-integration exemption) — synthesizes exactly one
/// keystroke combo and delegates every decision to the pure logic above.
pub struct EnigoPaste;

impl PasteSynthesizer for EnigoPaste {
    fn synthesize_paste(&self) -> io::Result<()> {
        use enigo::{Direction, Enigo, Key, Keyboard, Settings};

        let mut enigo =
            Enigo::new(&Settings::default()).map_err(|e| io::Error::other(e.to_string()))?;

        #[cfg(target_os = "macos")]
        let modifier = Key::Meta;
        #[cfg(not(target_os = "macos"))]
        let modifier = Key::Control;

        enigo
            .key(modifier, Direction::Press)
            .map_err(|e| io::Error::other(e.to_string()))?;
        enigo
            .key(Key::Unicode('v'), Direction::Click)
            .map_err(|e| io::Error::other(e.to_string()))?;
        enigo
            .key(modifier, Direction::Release)
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(())
    }
}

/// Selects which output target a finished dictation is routed to (AC-14
/// switches this per-dictation from settings-derived state).
pub enum OutputMode {
    /// Clipboard-swap + synthesized paste into the focused app (AC-9).
    CursorPaste,
    /// Templated append to a file, confined to `base_dir` (AC-3/AC-11, plus
    /// the path-confinement security AC carried from PR #41's Sentinel
    /// review into issue #21).
    File {
        base_dir: PathBuf,
        config: FileConfig,
    },
}

/// What happened after [`route`] dispatched a finished dictation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputOutcome {
    Pasted,
    AppendedTo(PathBuf),
}

/// Errors [`route`] can return, spanning both output targets.
#[derive(Debug, PartialEq, Eq)]
pub enum RouteError {
    Io(String),
    PathConfinement(PathConfinementError),
}

impl From<io::Error> for RouteError {
    fn from(err: io::Error) -> Self {
        RouteError::Io(err.to_string())
    }
}

impl From<PathConfinementError> for RouteError {
    fn from(err: PathConfinementError) -> Self {
        RouteError::PathConfinement(err)
    }
}

/// Dispatch a finished dictation's transcript to whichever target `mode`
/// selects (ADR-0002: the output router's job). Pure dispatch logic: OS
/// calls only ever happen inside the injected `clipboard`/`paste`/`sleep`
/// seams (cursor-paste branch) or inside `append_entry`'s `std::fs` calls
/// (file branch) — both fakeable in tests, as the tests above do.
#[allow(clippy::too_many_arguments)]
pub fn route(
    mode: &OutputMode,
    transcript: String,
    clock: Clock,
    clipboard: &impl Clipboard,
    paste: &impl PasteSynthesizer,
    sleep: impl FnOnce(Duration),
    restore_delay: Duration,
) -> Result<OutputOutcome, RouteError> {
    match mode {
        OutputMode::CursorPaste => {
            paste_via_clipboard_swap(
                clipboard,
                paste,
                sleep,
                ClipboardPayload::new(transcript),
                restore_delay,
            )?;
            Ok(OutputOutcome::Pasted)
        }
        OutputMode::File { base_dir, config } => {
            let expanded = expand_template(&config.path_template, clock);
            let confined = confine_relative_path(base_dir, &expanded)?;
            let resolved_config = FileConfig {
                path_template: confined.to_string_lossy().into_owned(),
                timestamp_prefix_template: config.timestamp_prefix_template.clone(),
            };
            let path = append_entry(&resolved_config, &transcript, clock)?;
            Ok(OutputOutcome::AppendedTo(path))
        }
    }
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
    use std::cell::RefCell;
    use std::fs;
    use tempfile::tempdir;

    /// Fake clipboard for tests: an in-memory cell, no real OS clipboard
    /// access.
    struct FakeClipboard {
        contents: RefCell<String>,
    }

    impl FakeClipboard {
        fn new(initial: &str) -> Self {
            Self {
                contents: RefCell::new(initial.to_string()),
            }
        }
    }

    impl Clipboard for FakeClipboard {
        fn get(&self) -> io::Result<String> {
            Ok(self.contents.borrow().clone())
        }

        fn set(&self, contents: &str) -> io::Result<()> {
            *self.contents.borrow_mut() = contents.to_string();
            Ok(())
        }
    }

    /// Fake paste synthesizer: records that it was called but never touches
    /// the clipboard itself (mirroring the real enigo glue, which only
    /// synthesizes a keystroke — the focused app, not our code, reads the
    /// clipboard in response).
    struct FakePaste {
        called: RefCell<bool>,
    }

    impl FakePaste {
        fn new() -> Self {
            Self {
                called: RefCell::new(false),
            }
        }
    }

    impl PasteSynthesizer for FakePaste {
        fn synthesize_paste(&self) -> io::Result<()> {
            *self.called.borrow_mut() = true;
            Ok(())
        }
    }

    #[test]
    fn clipboard_swap_paste_restores_pre_dictation_contents_ac9() {
        let clipboard = FakeClipboard::new("pre-dictation clipboard contents");
        let paste = FakePaste::new();
        let payload = ClipboardPayload::new("the dictated transcript".to_string());

        paste_via_clipboard_swap(
            &clipboard,
            &paste,
            |_delay| {},
            payload,
            Duration::from_millis(200),
        )
        .unwrap();

        assert!(*paste.called.borrow());
        assert_eq!(clipboard.get().unwrap(), "pre-dictation clipboard contents");
    }

    /// A paste synthesizer that always fails — simulates `enigo` failing on
    /// first-run macOS before Accessibility permission is granted (issue
    /// #65).
    struct FailingPaste;

    impl PasteSynthesizer for FailingPaste {
        fn synthesize_paste(&self) -> io::Result<()> {
            Err(io::Error::other(
                "synthetic paste synthesis failure (simulates enigo failing before Accessibility is granted)",
            ))
        }
    }

    #[test]
    fn clipboard_is_restored_when_paste_synthesis_fails_issue_65() {
        // Issue #65 (Sentinel 🔴-when-wired): a paste-synthesis failure must
        // NOT leave the transcript permanently on the clipboard. Before the
        // fix, the `?` on `paste.synthesize_paste()` returned early and
        // skipped the restore entirely — this discriminating assertion
        // checks the actual clipboard contents afterward, not merely that
        // an Err was returned.
        let clipboard = FakeClipboard::new("pre-dictation clipboard contents");
        let paste = FailingPaste;
        let payload = ClipboardPayload::new("the dictated transcript".to_string());

        let err = paste_via_clipboard_swap(
            &clipboard,
            &paste,
            |_delay| {},
            payload,
            Duration::from_millis(200),
        )
        .unwrap_err();

        assert_eq!(err.to_string().contains("synthetic paste synthesis"), true);
        assert_eq!(
            clipboard.get().unwrap(),
            "pre-dictation clipboard contents",
            "the transcript must not be left on the clipboard when paste synthesis fails"
        );
    }

    /// A fake clipboard whose `get()` fails on its *second* call onward —
    /// exercises the restore-on-all-error-paths requirement (issue #65) for
    /// the post-paste observation read, not just the paste-synthesis
    /// failure above.
    struct FlakyObserveClipboard {
        contents: RefCell<String>,
        calls: RefCell<u32>,
    }

    impl FlakyObserveClipboard {
        fn new(initial: &str) -> Self {
            Self {
                contents: RefCell::new(initial.to_string()),
                calls: RefCell::new(0),
            }
        }
    }

    impl Clipboard for FlakyObserveClipboard {
        fn get(&self) -> io::Result<String> {
            let mut calls = self.calls.borrow_mut();
            *calls += 1;
            if *calls >= 2 {
                return Err(io::Error::other("clipboard read failed"));
            }
            Ok(self.contents.borrow().clone())
        }

        fn set(&self, contents: &str) -> io::Result<()> {
            *self.contents.borrow_mut() = contents.to_string();
            Ok(())
        }
    }

    #[test]
    fn clipboard_is_restored_when_the_post_paste_observation_read_fails_issue_65() {
        // Issue #65: even when the *second* clipboard read (the one used to
        // decide whether to restore) fails, the pre-dictation contents must
        // still be restored best-effort, and the original error must still
        // propagate to the caller.
        let clipboard = FlakyObserveClipboard::new("pre-dictation clipboard contents");
        let paste = FakePaste::new();
        let payload = ClipboardPayload::new("the dictated transcript".to_string());

        let err = paste_via_clipboard_swap(
            &clipboard,
            &paste,
            |_delay| {},
            payload,
            Duration::from_millis(200),
        )
        .unwrap_err();

        assert_eq!(err.to_string(), "clipboard read failed");
        assert_eq!(
            clipboard.contents.borrow().as_str(),
            "pre-dictation clipboard contents",
            "the transcript must not be left on the clipboard when the post-paste read fails"
        );
    }

    #[test]
    fn clipboard_swap_paste_skips_restore_if_clipboard_changed_during_delay() {
        let clipboard = FakeClipboard::new("pre-dictation clipboard contents");
        let paste = FakePaste::new();
        let payload = ClipboardPayload::new("the dictated transcript".to_string());

        // Simulate another actor writing to the clipboard during the
        // restore delay, from inside the injected sleep callback.
        paste_via_clipboard_swap(
            &clipboard,
            &paste,
            |_delay| {
                clipboard
                    .set("someone else's newer clipboard value")
                    .unwrap();
            },
            payload,
            Duration::from_millis(200),
        )
        .unwrap();

        assert_eq!(
            clipboard.get().unwrap(),
            "someone else's newer clipboard value"
        );
    }

    #[test]
    fn route_dispatches_cursor_paste_and_restores_clipboard_ac9() {
        let clipboard = FakeClipboard::new("pre-dictation clipboard contents");
        let paste = FakePaste::new();

        let outcome = route(
            &OutputMode::CursorPaste,
            "the dictated transcript".to_string(),
            clock(2026, 7, 7, 9, 5),
            &clipboard,
            &paste,
            |_delay| {},
            Duration::from_millis(200),
        )
        .unwrap();

        assert_eq!(outcome, OutputOutcome::Pasted);
        assert!(*paste.called.borrow());
        assert_eq!(clipboard.get().unwrap(), "pre-dictation clipboard contents");
    }

    #[test]
    fn route_dispatches_file_mode_and_confines_the_path() {
        let dir = tempdir().unwrap();
        let clipboard = FakeClipboard::new("untouched");
        let paste = FakePaste::new();
        let mode = OutputMode::File {
            base_dir: dir.path().to_path_buf(),
            config: FileConfig {
                path_template: "daily/{{date:YYYY-MM-DD}}.md".to_string(),
                timestamp_prefix_template: Some("{{time:HH:mm}} ".to_string()),
            },
        };

        let outcome = route(
            &mode,
            "routed entry".to_string(),
            clock(2026, 7, 7, 9, 5),
            &clipboard,
            &paste,
            |_delay| {},
            Duration::from_millis(200),
        )
        .unwrap();

        let expected_path = dir.path().join("daily/2026-07-07.md");
        assert_eq!(outcome, OutputOutcome::AppendedTo(expected_path.clone()));
        assert_eq!(
            fs::read_to_string(&expected_path).unwrap(),
            "09:05 routed entry\n"
        );
        // File mode must never touch the clipboard.
        assert_eq!(clipboard.get().unwrap(), "untouched");
        assert!(!*paste.called.borrow());
    }

    #[test]
    fn route_rejects_a_file_template_that_resolves_absolute() {
        let dir = tempdir().unwrap();
        let clipboard = FakeClipboard::new("untouched");
        let paste = FakePaste::new();
        let mode = OutputMode::File {
            base_dir: dir.path().to_path_buf(),
            config: FileConfig {
                path_template: "/etc/passwd".to_string(),
                timestamp_prefix_template: None,
            },
        };

        let err = route(
            &mode,
            "entry".to_string(),
            clock(2026, 7, 7, 9, 5),
            &clipboard,
            &paste,
            |_delay| {},
            Duration::from_millis(200),
        )
        .unwrap_err();

        assert_eq!(
            err,
            RouteError::PathConfinement(PathConfinementError::AbsolutePath)
        );
    }

    #[test]
    fn route_rejects_a_file_template_that_escapes_the_base_dir() {
        let dir = tempdir().unwrap();
        let clipboard = FakeClipboard::new("untouched");
        let paste = FakePaste::new();
        let mode = OutputMode::File {
            base_dir: dir.path().to_path_buf(),
            config: FileConfig {
                path_template: "../../etc/{{date:YYYY-MM-DD}}.md".to_string(),
                timestamp_prefix_template: None,
            },
        };

        let err = route(
            &mode,
            "entry".to_string(),
            clock(2026, 7, 7, 9, 5),
            &clipboard,
            &paste,
            |_delay| {},
            Duration::from_millis(200),
        )
        .unwrap_err();

        assert_eq!(
            err,
            RouteError::PathConfinement(PathConfinementError::EscapesBaseDir)
        );
    }

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
