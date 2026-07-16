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
//! carried from PR #41's Sentinel review). The write itself is additionally
//! hardened against the symlink/TOCTOU gap (issue #208,
//! `open_confined_for_append`): the resolved parent is canonicalized and
//! verified to stay under the canonicalized base, and the final component is
//! opened refusing to follow a symlink (`O_NOFOLLOW` on Unix). This is
//! same-user-bounded defense-in-depth, not a privilege boundary — see that
//! function's docs. Restrictive file permissions on the confined target
//! remain a noted follow-up, not addressed here.
//!
//! `route`'s first non-test consumer is `pipeline` (issue #25), which calls
//! it from `Pipeline::run`; the runtime wiring in `lib.rs` (issue #91) then
//! drives `Pipeline::run` on a completed dictation, so `SystemClipboard`/
//! `EnigoPaste` are now live. `dead_code` stays allowed at module scope for
//! any item not yet reached from those call sites (e.g. surface kept for the
//! M2 settings UI).
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
/// Purely lexical: no filesystem access and no symlink resolution. The
/// symlink/TOCTOU hardening the lexical check can't provide lives in
/// [`open_confined_for_append`] (issue #208), which the file-route write
/// path runs on top of this. Restrictive file permissions on the confined
/// target remain a follow-up per issue #21's comment.
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

    // Issue #65 (Sentinel 🔴-when-wired): from this point on, the clipboard
    // holds the transcript, not the user's pre-dictation contents. Every
    // exit path below — the paste synthesizer failing (e.g. enigo failing
    // on first-run macOS before Accessibility is granted) or the final
    // observation read failing — must restore `saved` before returning,
    // rather than propagating the error via `?` and leaving the transcript
    // permanently on the clipboard. The restore itself is best-effort: its
    // own failure must never mask the original error being propagated.
    if let Err(paste_err) = paste.synthesize_paste() {
        let _ = clipboard.set(&saved);
        return Err(paste_err);
    }

    sleep(restore_delay);

    match clipboard.get() {
        Ok(observed) => {
            if should_restore_clipboard(&transcript, &observed) {
                clipboard.set(&saved)?;
            }
            Ok(())
        }
        Err(observe_err) => {
            let _ = clipboard.set(&saved);
            Err(observe_err)
        }
    }
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

/// The modifier key synthesized alongside `V` for a platform's native paste
/// shortcut: `Cmd+V` on macOS, `Ctrl+V` everywhere else (Windows, Linux).
/// Pure, `cfg`-selected lookup — no `enigo` call, no OS handle — so the
/// per-platform choice is unit-tested directly (issue #98) rather than only
/// implied by an inline `#[cfg]` inside [`EnigoPaste::synthesize_paste`].
/// Exactly one of the two `cfg`-gated definitions below is compiled for any
/// given target.
#[cfg(target_os = "macos")]
pub const fn paste_modifier() -> enigo::Key {
    enigo::Key::Meta
}

/// Windows/Linux variant of [`paste_modifier`] — see its doc comment above.
#[cfg(not(target_os = "macos"))]
pub const fn paste_modifier() -> enigo::Key {
    enigo::Key::Control
}

/// Real synthetic Cmd+V (macOS) / Ctrl+V (Windows, Linux) via `enigo`. Thin
/// OS glue (AGENTS.md OS-integration exemption) — synthesizes exactly one
/// keystroke combo, using [`paste_modifier`] (pure, unit-tested) to pick the
/// modifier; no decision logic lives in this impl.
pub struct EnigoPaste;

impl PasteSynthesizer for EnigoPaste {
    fn synthesize_paste(&self) -> io::Result<()> {
        use enigo::{Direction, Enigo, Key, Keyboard, Settings};

        let mut enigo =
            Enigo::new(&Settings::default()).map_err(|e| io::Error::other(e.to_string()))?;

        let modifier = paste_modifier();

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

/// Resolve the file-mode base directory a dictation's templated path
/// should be confined to (issue #180's settings-window picker — the "base
/// folder / vault" field, e.g. an Obsidian vault path). A blank
/// `configured` value (the pre-#180 default, or a user who cleared the
/// field) falls back to `app_data_dir`, preserving the previous hard-coded
/// behavior; a non-blank value is the user's chosen folder, used verbatim
/// (surrounding whitespace trimmed). Pure so the decision is unit-testable
/// without a live `tauri::AppHandle` —
/// `lib.rs::run_pipeline_in_background` (OS-integration glue) is the sole
/// real caller.
pub fn resolve_base_dir(configured: &str, app_data_dir: &Path) -> PathBuf {
    let trimmed = configured.trim();
    if trimmed.is_empty() {
        app_data_dir.to_path_buf()
    } else {
        PathBuf::from(trimmed)
    }
}

/// `O_NOFOLLOW` for the final `open(2)` component (issue #208). The standard
/// library exposes no constant for it, so we define the platform value
/// directly (std-only, **no new dependency**) and pass it through
/// [`std::os::unix::fs::OpenOptionsExt::custom_flags`]. Values match the
/// system `<fcntl.h>`: `0x0100` on the Darwin/BSD family, `0o400000` on
/// Linux/Android (asm-generic — the arches bla ships on). Any other Unix
/// gets `0` (a no-op custom flag); there the cross-platform pre-open
/// [`std::fs::symlink_metadata`] refusal and the canonical-parent check in
/// [`open_confined_for_append`] still apply.
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
const FINAL_OPEN_NOFOLLOW: i32 = 0x0100;

/// Linux/Android value of [`FINAL_OPEN_NOFOLLOW`] — see its doc comment.
#[cfg(any(target_os = "linux", target_os = "android"))]
const FINAL_OPEN_NOFOLLOW: i32 = 0o400000;

/// Fallback for any other Unix: a no-op custom flag — see [`FINAL_OPEN_NOFOLLOW`].
#[cfg(all(
    unix,
    not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "linux",
        target_os = "android"
    ))
))]
const FINAL_OPEN_NOFOLLOW: i32 = 0;

/// Open `path` for create-if-absent append, refusing to follow a symlink at
/// the **final** component. On Unix this is atomic via `O_NOFOLLOW`
/// ([`FINAL_OPEN_NOFOLLOW`]); the [`open_confined_for_append`] caller adds a
/// cross-platform pre-open [`std::fs::symlink_metadata`] refusal for the
/// non-race case (the documented Windows equivalent, since std wires no
/// `O_NOFOLLOW` there — and Windows symlink creation needs elevation, so the
/// residual final-component race is not reachable by an unprivileged
/// same-user process by default). OS-integration glue (AGENTS.md exemption).
#[cfg(unix)]
fn open_final_no_follow(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .custom_flags(FINAL_OPEN_NOFOLLOW)
        .open(path)
}

/// Non-Unix [`open_final_no_follow`]: std exposes no `O_NOFOLLOW` equivalent
/// here, so the final-component guard is the caller's pre-open
/// [`std::fs::symlink_metadata`] refusal — see its doc comment.
#[cfg(not(unix))]
fn open_final_no_follow(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new().create(true).append(true).open(path)
}

/// Pure prefix check backing the symlink-escape guard (issue #208): true iff
/// `candidate_canonical` is `base_canonical` itself or lies beneath it.
/// Component-wise (via [`Path::starts_with`]), so a sibling that merely
/// shares a string prefix (`/vault-evil` vs `/vault`) is correctly rejected.
/// Both arguments are expected already-canonicalized by the caller.
fn is_within_base(base_canonical: &Path, candidate_canonical: &Path) -> bool {
    candidate_canonical.starts_with(base_canonical)
}

/// Open the confined file-mode target for append, hardened against the
/// symlink/TOCTOU gap (issue #208, Sentinel SNTL-20260715-bla-PR204). Steps:
///
/// 1. `create_dir_all` the parent chain — this also materializes `base_dir`,
///    since [`confine_relative_path`] guarantees `confined` is lexically
///    under it.
/// 2. Canonicalize the resolved parent **and** `base_dir` (after step 1, so
///    every symlink in the chain is resolved) and verify the parent is still
///    within the base via [`is_within_base`] — catches a symlinked
///    *intermediate* directory that escapes the confined tree.
/// 3. Refuse a pre-existing *final-component* symlink ([`std::fs::symlink_metadata`],
///    no-follow) — the cross-platform belt to the Unix `O_NOFOLLOW` open.
/// 4. Open with [`open_final_no_follow`].
///
/// Same-user-bounded defense-in-depth (see the module docs / PR body):
/// closes the reachable escape, with a documented residual TOCTOU window on
/// the parent components between canonicalize and open. OS-integration glue.
fn open_confined_for_append(base_dir: &Path, confined: &Path) -> Result<fs::File, RouteError> {
    let parent = confined.parent().unwrap_or(base_dir);
    if !parent.as_os_str().is_empty() {
        fs::create_dir_all(parent)?;
    }

    let base_canonical = fs::canonicalize(base_dir)?;
    let parent_canonical = fs::canonicalize(parent)?;
    if !is_within_base(&base_canonical, &parent_canonical) {
        return Err(RouteError::UnsafeSymlink);
    }

    if let Ok(meta) = fs::symlink_metadata(confined) {
        if meta.file_type().is_symlink() {
            return Err(RouteError::UnsafeSymlink);
        }
    }

    open_final_no_follow(confined).map_err(RouteError::from)
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
    /// The file-mode target could not be written safely: the resolved parent
    /// canonicalized to a path outside the confined base (a symlinked
    /// intermediate directory escaping the tree), or the final component was
    /// a symlink (issue #208). Carries **no** path/content — kind only, so it
    /// flows through the existing `PipelineError::Output` -> `ErrorKind::Other`
    /// surface without leaking anything (MISSION §7 no-log invariant).
    UnsafeSymlink,
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
            // Issue #208: `confine_relative_path` is purely lexical, so the
            // open must additionally refuse symlink escapes at write time —
            // `open_confined_for_append` verifies the canonical parent stays
            // under the canonical base and refuses to follow a symlinked
            // final component.
            let mut file = open_confined_for_append(base_dir, &confined)?;
            let prefix = config
                .timestamp_prefix_template
                .as_deref()
                .map(|t| expand_template(t, clock))
                .unwrap_or_default();
            writeln!(file, "{prefix}{transcript}")?;
            Ok(OutputOutcome::AppendedTo(confined))
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

    // Issue #208: refuse to follow a symlink at the final component (Unix:
    // atomic via `O_NOFOLLOW`), so a symlinked target can't redirect the
    // append out of tree. `route`'s file branch adds the base-confinement
    // and intermediate-symlink checks (`open_confined_for_append`) on top of
    // this; this open is the shared final-component guard.
    let mut file = open_final_no_follow(&path)?;
    writeln!(file, "{prefix}{entry}")?;

    Ok(path)
}

/// Compile-time proof that [`ClipboardPayload`] can never flow into a log
/// macro, string formatting, or a serializer (ADR-0003, PRD AC-9): if
/// `Debug`, `Display`, or `serde::Serialize` were ever added back to it,
/// this assertion fails to compile and `cargo test` fails with it.
#[cfg(test)]
mod clipboard_payload_trait_assertions {
    use super::{CapturedSelection, ClipboardPayload};
    use static_assertions::assert_not_impl_any;

    assert_not_impl_any!(ClipboardPayload: std::fmt::Debug, std::fmt::Display, serde::Serialize);
    // Issue #257: `CapturedSelection` carries the captured selection (and
    // the pre-copy clipboard) as `ClipboardPayload` fields — assert
    // explicitly, per the module docs' standing instruction to extend this
    // guard for every second payload-carrying type, rather than relying on
    // the fact that deriving these traits would already fail to compile
    // because `ClipboardPayload` itself doesn't implement them.
    assert_not_impl_any!(CapturedSelection: std::fmt::Debug, std::fmt::Display, serde::Serialize);
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

        assert!(err.to_string().contains("synthetic paste synthesis"));
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

    // -----------------------------------------------------------------
    // Issue #180: the settings-window picker's "base folder / vault" field
    // persists as `settings::Settings::file_base_dir`; `resolve_base_dir` is
    // the pure decision `lib.rs::run_pipeline_in_background` (OS-integration
    // glue, not unit-testable directly) delegates to when building
    // `OutputMode::File`'s `base_dir` — previously hard-coded to
    // `app_data_dir` with no way for the user to change it.
    // -----------------------------------------------------------------

    #[test]
    fn resolve_base_dir_falls_back_to_app_data_dir_when_unset() {
        let app_data_dir = Path::new("/Users/cofounder/Library/Application Support/bla");
        assert_eq!(
            resolve_base_dir("", app_data_dir),
            app_data_dir.to_path_buf()
        );
    }

    #[test]
    fn resolve_base_dir_falls_back_to_app_data_dir_when_blank() {
        let app_data_dir = Path::new("/Users/cofounder/Library/Application Support/bla");
        assert_eq!(
            resolve_base_dir("   ", app_data_dir),
            app_data_dir.to_path_buf()
        );
    }

    #[test]
    fn resolve_base_dir_uses_the_configured_vault_path_verbatim_when_set() {
        let app_data_dir = Path::new("/Users/cofounder/Library/Application Support/bla");
        let vault = "/Users/cofounder/Obsidian/Vault";
        assert_eq!(resolve_base_dir(vault, app_data_dir), PathBuf::from(vault));
    }

    #[test]
    fn resolve_base_dir_trims_surrounding_whitespace_off_a_configured_path() {
        let app_data_dir = Path::new("/app-data");
        assert_eq!(
            resolve_base_dir("  /Users/cofounder/Obsidian/Vault  ", app_data_dir),
            PathBuf::from("/Users/cofounder/Obsidian/Vault")
        );
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

    // -----------------------------------------------------------------
    // Issue #257 (AC-48): command-mode selection capture/replace, reusing
    // the clipboard-swap machinery above. The discriminating behavior under
    // test is the restore distinction: `replace_selection` must restore the
    // ORIGINAL pre-copy clipboard value `capture_selection` saved, never the
    // intermediate captured-selection value that sits on the clipboard while
    // the instruction is recorded/transformed in between.
    // -----------------------------------------------------------------

    /// Fake copy synthesizer: on `synthesize_copy`, writes a fixed
    /// "selection" string to the clipboard — mirroring the real `enigo`
    /// glue, where the synthesized keystroke doesn't write the clipboard
    /// itself; the OS does, in response to it, because some other app has
    /// focus and a selection. Records whether it was invoked.
    struct FakeCopy<'a> {
        clipboard: &'a FakeClipboard,
        selection_text: &'a str,
        called: RefCell<bool>,
    }

    impl<'a> FakeCopy<'a> {
        fn new(clipboard: &'a FakeClipboard, selection_text: &'a str) -> Self {
            Self {
                clipboard,
                selection_text,
                called: RefCell::new(false),
            }
        }
    }

    impl CopySynthesizer for FakeCopy<'_> {
        fn synthesize_copy(&self) -> io::Result<()> {
            *self.called.borrow_mut() = true;
            self.clipboard.set(self.selection_text)
        }
    }

    #[test]
    fn capture_selection_saves_pre_copy_clipboard_and_returns_the_selection_ac48() {
        let clipboard = FakeClipboard::new("pre-copy clipboard contents");
        let copy = FakeCopy::new(&clipboard, "captured selection text");

        let captured = capture_selection(&clipboard, &copy).unwrap();

        assert!(*copy.called.borrow());
        assert_eq!(captured.selection.into_inner(), "captured selection text");
        assert_eq!(
            captured.pre_copy_clipboard.into_inner(),
            "pre-copy clipboard contents"
        );
    }

    #[test]
    fn capture_selection_never_writes_the_clipboard_itself() {
        // Capture only ever *reads* the clipboard (before and after the
        // copy keystroke) — the OS is what writes the selection onto it in
        // response to the synthesized keystroke, not this function.
        let clipboard = FakeClipboard::new("pre-copy clipboard contents");
        let copy = FakeCopy::new(&clipboard, "captured selection text");

        capture_selection(&clipboard, &copy).unwrap();

        assert_eq!(
            clipboard.get().unwrap(),
            "captured selection text",
            "capture must not touch the clipboard again after reading the selection"
        );
    }

    #[test]
    fn replace_selection_restores_the_pre_copy_original_not_the_captured_selection_ac48() {
        // Simulate the state after `capture_selection` already ran: the
        // clipboard currently holds the mid-flow captured-selection value,
        // NOT the user's original pre-copy clipboard contents.
        let clipboard = FakeClipboard::new("captured selection text");
        let paste = FakePaste::new();
        let pre_copy_clipboard =
            ClipboardPayload::new("ORIGINAL pre-copy clipboard contents".to_string());
        let transformed = ClipboardPayload::new("transformed replacement text".to_string());

        replace_selection(
            &clipboard,
            &paste,
            |_delay| {},
            pre_copy_clipboard,
            transformed,
            Duration::from_millis(200),
        )
        .unwrap();

        assert!(*paste.called.borrow());
        assert_eq!(
            clipboard.get().unwrap(),
            "ORIGINAL pre-copy clipboard contents",
            "must restore the pre-copy original, not the intermediate captured-selection value"
        );
    }

    #[test]
    fn replace_selection_writes_the_transformed_text_before_pasting() {
        let clipboard = FakeClipboard::new("captured selection text");
        // A paste synthesizer that snapshots what the clipboard held at the
        // moment it was invoked, proving the transformed text was written
        // first.
        struct SnapshotPaste<'a> {
            clipboard: &'a FakeClipboard,
            seen: RefCell<Option<String>>,
        }
        impl PasteSynthesizer for SnapshotPaste<'_> {
            fn synthesize_paste(&self) -> io::Result<()> {
                *self.seen.borrow_mut() = Some(self.clipboard.get().unwrap());
                Ok(())
            }
        }
        let paste = SnapshotPaste {
            clipboard: &clipboard,
            seen: RefCell::new(None),
        };
        let pre_copy_clipboard = ClipboardPayload::new("original".to_string());
        let transformed = ClipboardPayload::new("transformed replacement text".to_string());

        replace_selection(
            &clipboard,
            &paste,
            |_delay| {},
            pre_copy_clipboard,
            transformed,
            Duration::from_millis(200),
        )
        .unwrap();

        assert_eq!(
            paste.seen.borrow().as_deref(),
            Some("transformed replacement text")
        );
    }

    #[test]
    fn replace_selection_skips_restore_if_clipboard_changed_during_delay() {
        let clipboard = FakeClipboard::new("captured selection text");
        let paste = FakePaste::new();
        let pre_copy_clipboard = ClipboardPayload::new("original pre-copy contents".to_string());
        let transformed = ClipboardPayload::new("transformed replacement text".to_string());

        // Simulate another actor writing to the clipboard during the
        // restore delay, from inside the injected sleep callback.
        replace_selection(
            &clipboard,
            &paste,
            |_delay| {
                clipboard
                    .set("someone else's newer clipboard value")
                    .unwrap();
            },
            pre_copy_clipboard,
            transformed,
            Duration::from_millis(200),
        )
        .unwrap();

        assert_eq!(
            clipboard.get().unwrap(),
            "someone else's newer clipboard value"
        );
    }

    #[test]
    fn replace_selection_restores_pre_copy_original_when_paste_synthesis_fails() {
        // Mirrors issue #65's coverage for `paste_via_clipboard_swap`: a
        // paste-synthesis failure must not leave the transformed text
        // permanently on the clipboard, and the value restored must be the
        // pre-copy original passed in, not whatever happened to be on the
        // clipboard beforehand.
        let clipboard = FakeClipboard::new("captured selection text");
        let paste = FailingPaste;
        let pre_copy_clipboard = ClipboardPayload::new("original pre-copy contents".to_string());
        let transformed = ClipboardPayload::new("transformed replacement text".to_string());

        let err = replace_selection(
            &clipboard,
            &paste,
            |_delay| {},
            pre_copy_clipboard,
            transformed,
            Duration::from_millis(200),
        )
        .unwrap_err();

        assert!(err.to_string().contains("synthetic paste synthesis"));
        assert_eq!(
            clipboard.get().unwrap(),
            "original pre-copy contents",
            "must not leave the transformed text on the clipboard when paste synthesis fails"
        );
    }

    // -----------------------------------------------------------------
    // Issue #98: paste modifier is a pure, cfg-selected lookup — Cmd on
    // macOS, Ctrl everywhere else (Windows, Linux) — so it's asserted here
    // rather than only inline inside `EnigoPaste::synthesize_paste`.
    // -----------------------------------------------------------------

    #[test]
    fn paste_modifier_matches_this_platforms_native_shortcut() {
        #[cfg(target_os = "macos")]
        assert_eq!(
            paste_modifier(),
            enigo::Key::Meta,
            "macOS pastes with Cmd+V, not Ctrl+V"
        );

        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            paste_modifier(),
            enigo::Key::Control,
            "Windows/Linux paste with Ctrl+V, not Cmd+V"
        );
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

    // -----------------------------------------------------------------
    // Issue #98: a file-mode template with literal `/` separators (as a
    // user would type into settings — this app's templating convention
    // always uses `/`, never the host OS's separator) must still create the
    // right nested directories and append correctly. Built by string
    // concatenation with a hard-coded `/`, deliberately *not*
    // `Path::join` (which would silently use the host separator and defeat
    // the point), so this exercises the same literal-`/` string a Windows
    // build would receive from settings.
    // -----------------------------------------------------------------

    #[test]
    fn append_entry_creates_nested_dirs_from_a_literal_forward_slash_template_issue_98() {
        let dir = tempdir().unwrap();
        let template = format!(
            "{}/sub/dir/{{{{date:YYYY-MM-DD}}}}.md",
            dir.path().display()
        );
        let config = FileConfig {
            path_template: template,
            timestamp_prefix_template: None,
        };

        let written = append_entry(&config, "literal slash entry", clock(2026, 7, 7, 9, 5))
            .expect("append_entry must succeed for a literal-'/' template");

        let expected = dir.path().join("sub").join("dir").join("2026-07-07.md");
        assert_eq!(written, expected);
        assert!(
            dir.path().join("sub").join("dir").is_dir(),
            "intermediate directories from the literal-'/' template must be created"
        );
        assert_eq!(
            fs::read_to_string(&expected).unwrap(),
            "literal slash entry\n"
        );
    }

    #[test]
    fn append_entry_creates_nested_dirs_from_a_slash_date_token_plus_trailing_segment_issue_98() {
        let dir = tempdir().unwrap();
        // `{{date:YYYY/MM/DD}}` expands to a string containing its own `/`
        // separators, followed by a literal `/note.md` segment — two
        // sources of `/` compounding in one template.
        let template = format!("{}/{{{{date:YYYY/MM/DD}}}}/note.md", dir.path().display());
        let config = FileConfig {
            path_template: template,
            timestamp_prefix_template: Some("{{time:HH:mm}} ".to_string()),
        };

        let written = append_entry(&config, "compound slash entry", clock(2026, 12, 3, 23, 59))
            .expect("append_entry must succeed for a slash-producing date token");

        let expected = dir
            .path()
            .join("2026")
            .join("12")
            .join("03")
            .join("note.md");
        assert_eq!(written, expected);
        assert!(
            dir.path().join("2026").join("12").join("03").is_dir(),
            "every intermediate directory implied by the date token's '/'s must be created"
        );
        assert_eq!(
            fs::read_to_string(&expected).unwrap(),
            "23:59 compound slash entry\n"
        );
    }

    // -----------------------------------------------------------------
    // Issue #208 (Sentinel SNTL-20260715-bla-PR204 🟡, sentinel:security):
    // harden the file-output write path against the symlink/TOCTOU gap.
    // `confine_relative_path` is purely lexical, so a symlink swapped in (or
    // pre-planted inside the user-chosen base dir) between confine and write
    // can redirect the append outside the confined tree. The fix:
    //   * canonicalize the resolved parent (after `create_dir_all`) and
    //     verify it stays under the canonicalized base — catches a symlinked
    //     *intermediate* directory escaping the tree;
    //   * refuse a pre-existing *final-component* symlink; and
    //   * open the final component with `O_NOFOLLOW` on Unix (atomic).
    // Same-user-bounded, defense-in-depth (see module docs / PR body).
    // -----------------------------------------------------------------

    #[test]
    fn is_within_base_accepts_paths_under_the_canonical_base() {
        let base = Path::new("/private/vault");
        assert!(is_within_base(base, Path::new("/private/vault")));
        assert!(is_within_base(base, Path::new("/private/vault/daily")));
        assert!(is_within_base(base, Path::new("/private/vault/a/b/c.md")));
    }

    #[test]
    fn is_within_base_rejects_paths_outside_the_canonical_base() {
        let base = Path::new("/private/vault");
        assert!(!is_within_base(base, Path::new("/private")));
        assert!(!is_within_base(base, Path::new("/etc/passwd")));
        assert!(!is_within_base(base, Path::new("/private/other")));
        // A sibling that shares a *string* prefix but not a *path* prefix
        // must not be accepted (the check is component-wise, not textual).
        assert!(!is_within_base(base, Path::new("/private/vault-evil/x.md")));
    }

    #[cfg(unix)]
    #[test]
    fn route_refuses_to_append_through_a_symlinked_intermediate_dir_escaping_base() {
        use std::os::unix::fs::symlink;

        let base = tempdir().unwrap();
        let outside = tempdir().unwrap();
        // Pre-plant: `base/daily` is a symlink to a directory OUTSIDE the
        // confined tree — exactly the "symlink pre-planted inside the
        // user-chosen base dir" case from the finding.
        symlink(outside.path(), base.path().join("daily")).unwrap();

        let clipboard = FakeClipboard::new("untouched");
        let paste = FakePaste::new();
        let mode = OutputMode::File {
            base_dir: base.path().to_path_buf(),
            config: FileConfig {
                path_template: "daily/{{date:YYYY-MM-DD}}.md".to_string(),
                timestamp_prefix_template: None,
            },
        };

        let err = route(
            &mode,
            "secret dictation".to_string(),
            clock(2026, 7, 7, 9, 5),
            &clipboard,
            &paste,
            |_delay| {},
            Duration::from_millis(200),
        )
        .unwrap_err();

        assert_eq!(err, RouteError::UnsafeSymlink);
        assert!(
            !outside.path().join("2026-07-07.md").exists(),
            "append escaped the confined base via a symlinked intermediate dir"
        );
    }

    #[cfg(unix)]
    #[test]
    fn route_refuses_to_append_when_the_final_target_is_a_symlink() {
        use std::os::unix::fs::symlink;

        let base = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let victim = outside.path().join("victim.md");
        fs::write(&victim, "original contents\n").unwrap();
        // Pre-plant a final-component symlink inside the base that points at
        // a file outside it.
        symlink(&victim, base.path().join("2026-07-07.md")).unwrap();

        let mode = OutputMode::File {
            base_dir: base.path().to_path_buf(),
            config: FileConfig {
                path_template: "{{date:YYYY-MM-DD}}.md".to_string(),
                timestamp_prefix_template: None,
            },
        };

        let err = route(
            &mode,
            "secret dictation".to_string(),
            clock(2026, 7, 7, 9, 5),
            &FakeClipboard::new("untouched"),
            &FakePaste::new(),
            |_delay| {},
            Duration::from_millis(200),
        )
        .unwrap_err();

        assert_eq!(err, RouteError::UnsafeSymlink);
        assert_eq!(
            fs::read_to_string(&victim).unwrap(),
            "original contents\n",
            "append followed a final-component symlink out of the confined base"
        );
    }

    #[cfg(unix)]
    #[test]
    fn route_still_appends_when_the_base_dir_itself_is_a_symlink() {
        use std::os::unix::fs::symlink;

        // A user may legitimately point `base_dir` at a symlink (e.g. an
        // Obsidian vault reachable through one). Canonicalizing the base
        // before comparing means this must NOT be over-refused.
        let real = tempdir().unwrap();
        let link_parent = tempdir().unwrap();
        let base_link = link_parent.path().join("vault-link");
        symlink(real.path(), &base_link).unwrap();

        let mode = OutputMode::File {
            base_dir: base_link.clone(),
            config: FileConfig {
                path_template: "daily/{{date:YYYY-MM-DD}}.md".to_string(),
                timestamp_prefix_template: Some("{{time:HH:mm}} ".to_string()),
            },
        };

        let outcome = route(
            &mode,
            "legit entry".to_string(),
            clock(2026, 7, 7, 9, 5),
            &FakeClipboard::new("untouched"),
            &FakePaste::new(),
            |_delay| {},
            Duration::from_millis(200),
        )
        .unwrap();

        match outcome {
            OutputOutcome::AppendedTo(_) => {}
            other => panic!("expected AppendedTo, got {other:?}"),
        }
        // Written through the symlinked base into the real directory.
        assert_eq!(
            fs::read_to_string(real.path().join("daily/2026-07-07.md")).unwrap(),
            "09:05 legit entry\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn append_entry_refuses_to_follow_a_final_component_symlink() {
        use std::os::unix::fs::symlink;

        // `append_entry` opens its final component with `O_NOFOLLOW`, so a
        // symlinked target must make the open fail rather than writing
        // through it. (Without the fix, the append would overwrite/append to
        // the symlink's out-of-tree target.)
        let base = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let victim = outside.path().join("victim.md");
        fs::write(&victim, "original contents\n").unwrap();
        let link = base.path().join("note.md");
        symlink(&victim, &link).unwrap();

        let config = FileConfig {
            path_template: link.to_string_lossy().into_owned(),
            timestamp_prefix_template: None,
        };

        let result = append_entry(&config, "should not be written", clock(2026, 7, 7, 9, 5));

        assert!(
            result.is_err(),
            "append_entry must refuse to follow a final-component symlink"
        );
        assert_eq!(
            fs::read_to_string(&victim).unwrap(),
            "original contents\n",
            "append_entry followed a final-component symlink to an out-of-tree file"
        );
    }
}
