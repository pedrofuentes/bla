//! The `Cleanup` trait and its implementations: `RegexCleanup` (always available)
//! and `OllamaCleanup` (LLM pass via `localhost:11434`, rewrite-only prompts).
//!
//! Pure logic — no OS calls, fully unit-testable, TDD-mandatory (AGENTS.md).
//! `OllamaCleanup` falls back to `RegexCleanup` whenever Ollama is unreachable,
//! so the pipeline never surfaces a cleanup error to the output path (MISSION AC-4).
//!
//! Prompts live in `src-tauri/prompts/` as versioned files with fixture-based
//! regression checks — never inlined here.
//!
//! This module defines the `Cleanup` trait, `Tone`, `CleanupError`, the
//! `RegexCleanup` baseline (ADR-0005, PRD AC-4), and `OllamaCleanup`, the
//! optional LLM pass (issue #20, PRD AC-4/AC-10). `OllamaCleanup`'s HTTP
//! transport is injected behind the `OllamaTransport` trait so request
//! shaping, response parsing, and the unreachable-fallback decision are
//! pure and unit-tested without a network call or a running Ollama
//! instance; only `UreqTransport::post` touches a real socket, and only
//! ever the configured `localhost:11434`-by-default origin (MISSION §5).
//!
//! `mod cleanup` isn't `pub` and `commands.rs` doesn't call into it yet — that
//! wiring lands with the pipeline-integration work (issue #25 and friends),
//! including the dispatch that catches `CleanupError::Unreachable` from
//! `OllamaCleanup` and falls back to `RegexCleanup` (AC-4).
//! Until then this file's items are only reachable from its own unit tests,
//! so `dead_code` is silenced here rather than crate-wide.
#![allow(dead_code)]

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::OnceLock;
use std::time::Duration;

/// Controls how aggressively a [`Cleanup`] implementation rewrites the raw
/// transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Default cleanup pass: filler removal, spacing, capitalization,
    /// sentence-final punctuation (and, for LLM-backed implementations,
    /// self-correction resolution — see ADR-0005).
    Neutral,
    /// Bypasses cleanup entirely: the raw transcript is returned essentially
    /// untouched. Reserved for the M3 verbatim tone profile (PRD AC-22).
    Verbatim,
}

/// Errors a [`Cleanup`] implementation may return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupError {
    /// Reserved for the future `OllamaCleanup` (issue #20) — e.g. an
    /// unreachable or malformed LLM backend after fallback has been
    /// exhausted. `RegexCleanup` is pure, infallible logic and never returns
    /// this variant.
    Unreachable,
}

impl fmt::Display for CleanupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CleanupError::Unreachable => write!(f, "cleanup backend unreachable"),
        }
    }
}

impl std::error::Error for CleanupError {}

/// Pure text-transformation seam (ADR-0005). All rewriting hangs off this
/// trait so the pipeline can dispatch between `RegexCleanup` (always on) and
/// a future LLM-backed implementation without changing callers.
pub trait Cleanup {
    /// Rewrites `raw` according to `tone`. Implementations must be pure
    /// (no OS/network calls) with the sole exception of a future
    /// LLM-backed implementation's optional network probe, which must
    /// degrade to the regex baseline rather than propagate an error
    /// (MISSION AC-4).
    fn clean(&self, raw: &str, tone: Tone) -> Result<String, CleanupError>;
}

/// Deterministic, always-available baseline cleanup (ADR-0005, PRD AC-4).
///
/// Under [`Tone::Neutral`], `RegexCleanup`:
/// 1. Removes unambiguous filler interjections ("um", "uh", "er" —
///    word-boundary, case-insensitive) unconditionally.
/// 2. Removes "like" / "you know" **only** when comma-flanked on both sides
///    (e.g. "it's, like, great"), since that punctuation pattern cheaply and
///    reliably marks discourse-filler usage in speech transcripts. Other
///    occurrences — comparative ("looks like rain"), literal ("you know the
///    rules"), or sentence-initial/-final — are deliberately left alone:
///    telling those apart from genuine filler usage isn't cheap, so this
///    baseline stays conservative rather than risk stripping real content.
/// 3. Collapses runs of whitespace (including any left behind by 1–2) to a
///    single space and trims the ends.
/// 4. Capitalizes the first letter of the string and of every sentence that
///    follows a `.`, `!`, or `?`.
/// 5. Ensures the result ends with sentence-final punctuation (`.` added if
///    none of `.`/`!`/`?` is already present).
///
/// `RegexCleanup` does **not** resolve self-corrections (false starts,
/// "I mean", restart-and-rephrase) — that repair is reserved for the future
/// LLM pass (ADR-0005). It operates purely on tokens/whitespace and never
/// returns [`CleanupError::Unreachable`].
pub struct RegexCleanup;

impl Cleanup for RegexCleanup {
    fn clean(&self, raw: &str, tone: Tone) -> Result<String, CleanupError> {
        match tone {
            Tone::Verbatim => Ok(raw.to_string()),
            Tone::Neutral => Ok(clean_text(raw)),
        }
    }
}

struct FillerPatterns {
    /// Unambiguous filler interjections, plus a directly-trailing comma so
    /// removal doesn't leave orphaned punctuation behind.
    interjection: Regex,
    /// "like" / "you know" when comma-flanked on both sides — see
    /// `RegexCleanup`'s doc comment for why this is the chosen heuristic.
    comma_flanked_filler: Regex,
    /// Any run of whitespace, collapsed to a single space.
    whitespace: Regex,
}

fn patterns() -> &'static FillerPatterns {
    static PATTERNS: OnceLock<FillerPatterns> = OnceLock::new();
    PATTERNS.get_or_init(|| FillerPatterns {
        interjection: Regex::new(r"(?i)\b(?:um|uh|er)\b,?").expect("valid regex"),
        comma_flanked_filler: Regex::new(r"(?i),\s*(?:like|you know)\s*,").expect("valid regex"),
        whitespace: Regex::new(r"\s+").expect("valid regex"),
    })
}

/// The deterministic rewrite used by [`RegexCleanup`] under [`Tone::Neutral`].
/// See that type's doc comment for the exact transform order and rationale.
fn clean_text(raw: &str) -> String {
    let patterns = patterns();

    let without_interjections = patterns.interjection.replace_all(raw, "");
    let without_fillers = patterns
        .comma_flanked_filler
        .replace_all(&without_interjections, ",");
    let collapsed = patterns.whitespace.replace_all(&without_fillers, " ");
    let trimmed = collapsed.trim();

    if trimmed.is_empty() {
        return String::new();
    }

    let capitalized = capitalize_sentence_starts(trimmed);

    let ends_with_terminal = matches!(capitalized.chars().last(), Some('.' | '!' | '?'));
    if ends_with_terminal {
        capitalized
    } else {
        format!("{capitalized}.")
    }
}

/// Capitalizes the first letter of `s` and the first letter following every
/// `.`, `!`, or `?` (skipping any whitespace in between).
fn capitalize_sentence_starts(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;
    for c in s.chars() {
        if capitalize_next && c.is_alphabetic() {
            result.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            if c == '.' || c == '!' || c == '?' {
                capitalize_next = true;
            }
            result.push(c);
        }
    }
    result
}

/// Default Ollama origin (MISSION §5: the only permitted runtime origin
/// besides model download). Configurable per [`OllamaCleanup::new`] — e.g.
/// for a non-default port — but the constant here is what ships by default.
pub const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434";

/// The versioned, rewrite-only cleanup prompt (ADR-0005, MISSION §7,
/// PRD AC-10). Embedded at compile time from the versioned prompt file so
/// there is no runtime file path to resolve or fail to find; bumping the
/// prompt means adding `cleanup_v2.txt` and repointing this constant, never
/// editing `cleanup_v1.txt` in place.
pub const CLEANUP_PROMPT_V1: &str = include_str!("../prompts/cleanup_v1.txt");

/// Errors an [`OllamaTransport`] may return. `OllamaCleanup::clean` maps
/// every variant to [`CleanupError::Unreachable`] (AC-4) — transport
/// internals never propagate past this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The endpoint could not be reached at all (connection refused, DNS
    /// failure, ...).
    ConnectionFailed,
    /// The call was reachable but did not complete within the configured
    /// connect/read timeout (a hung-but-reachable endpoint). Kept distinct
    /// from [`Self::ConnectionFailed`] so a hung Ollama can't block the
    /// sync call forever; still maps to [`CleanupError::Unreachable`] so the
    /// AC-4 fallback fires.
    Timeout,
    /// The endpoint responded, but the body wasn't a response this module
    /// can parse.
    InvalidResponse,
}

/// Injected HTTP transport seam (ADR-0005). All of `OllamaCleanup`'s request
/// shaping, response parsing, and unreachable-fallback logic is pure and
/// tested against a stub implementation of this trait — the real,
/// network-touching implementation ([`UreqTransport`]) is thin glue with no
/// decision-making of its own.
pub trait OllamaTransport {
    /// POSTs the JSON-encoded `body` to `url` and returns the raw response
    /// body on success.
    fn post(&self, url: &str, body: &str) -> Result<String, TransportError>;
}

/// Default connect timeout for [`UreqTransport`] — how long to wait to
/// establish the TCP connection to Ollama before giving up (and falling
/// back to `RegexCleanup`).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Default read timeout for [`UreqTransport`] — how long to wait for the
/// model's response once connected. Generous because local generation can
/// take a few seconds, but bounded so a hung endpoint can't block forever.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// The real transport: a synchronous `ureq` POST over a preconfigured
/// [`ureq::Agent`]. Contains no logic beyond making the call and translating
/// its outcome to [`TransportError`] — by design, this is the only code in
/// the module that can open a socket, and it only ever talks to the URL it's
/// given (which `OllamaCleanup` builds from its configured,
/// localhost-by-default base URL — MISSION §5).
///
/// The agent is built with **`redirects(0)`** so a squatting responder that
/// answers with a 3xx can't bounce the request to another host — the
/// single-origin (localhost-only) egress invariant holds even under a
/// hostile local responder. Connect and read timeouts are set (caller-
/// configurable via [`Self::new`]) so a hung-but-reachable endpoint fails
/// with [`TransportError::Timeout`] instead of blocking the sync call
/// forever.
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    /// Builds a transport whose agent enforces `connect_timeout`,
    /// `read_timeout`, and no redirects.
    pub fn new(connect_timeout: Duration, read_timeout: Duration) -> Self {
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout_connect(connect_timeout)
            .timeout_read(read_timeout)
            .build();
        Self { agent }
    }
}

impl Default for UreqTransport {
    /// A transport with [`DEFAULT_CONNECT_TIMEOUT`] / [`DEFAULT_READ_TIMEOUT`].
    fn default() -> Self {
        Self::new(DEFAULT_CONNECT_TIMEOUT, DEFAULT_READ_TIMEOUT)
    }
}

impl OllamaTransport for UreqTransport {
    fn post(&self, url: &str, body: &str) -> Result<String, TransportError> {
        let response = self
            .agent
            .post(url)
            .set("Content-Type", "application/json")
            .send_string(body)
            .map_err(classify_ureq_error)?;
        response
            .into_string()
            .map_err(|_| TransportError::InvalidResponse)
    }
}

/// Best-effort classification of a `ureq` error into a [`TransportError`].
/// A non-2xx status or an unparsable/redirect response is treated as an
/// invalid response; a timeout is surfaced distinctly; anything else is a
/// connection failure. All three map to [`CleanupError::Unreachable`]
/// upstream, so this only affects diagnostics, never the fallback decision.
fn classify_ureq_error(err: ureq::Error) -> TransportError {
    match err {
        ureq::Error::Status(_, _) => TransportError::InvalidResponse,
        ureq::Error::Transport(transport) => {
            let msg = transport.to_string().to_lowercase();
            if msg.contains("timed out") || msg.contains("timeout") {
                TransportError::Timeout
            } else {
                TransportError::ConnectionFailed
            }
        }
    }
}

/// Request body shape for Ollama's `/api/generate` endpoint. `system`
/// carries the rewrite-only prompt ([`CLEANUP_PROMPT_V1`]); `prompt` carries
/// the raw transcript, untouched, so the model sees exactly the input the
/// caller passed in (AC-10's rewrite-only property extends to what this
/// module sends upstream, not just what it returns).
#[derive(Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    system: &'a str,
    prompt: &'a str,
    stream: bool,
}

/// The subset of Ollama's `/api/generate` response this module reads.
#[derive(Deserialize)]
struct GenerateResponse {
    response: String,
}

/// Optional LLM-backed cleanup pass over a local Ollama instance
/// (ADR-0005, PRD AC-4/AC-10). Under [`Tone::Neutral`], sends the raw
/// transcript to the configured endpoint alongside the versioned
/// rewrite-only prompt ([`CLEANUP_PROMPT_V1`]) and returns the model's
/// response verbatim (trimmed). [`Tone::Verbatim`] bypasses the transport
/// entirely, mirroring [`RegexCleanup`].
///
/// Never returns a transport error: any failure to reach or parse a
/// response from the endpoint is mapped to [`CleanupError::Unreachable`],
/// which the pipeline (issue #25) catches to fall back to [`RegexCleanup`]
/// with no error surfaced to the paste path (AC-4). This module never logs
/// transcript content (MISSION §5).
pub struct OllamaCleanup<T: OllamaTransport> {
    base_url: String,
    model: String,
    transport: T,
}

impl<T: OllamaTransport> OllamaCleanup<T> {
    /// Builds an `OllamaCleanup` against `base_url` (no trailing slash
    /// required — it's trimmed) using `model` and the given transport.
    ///
    /// MISSION §5 invariant: `base_url` must resolve to the local machine
    /// (`localhost`/`127.0.0.1`/`[::1]`) — Ollama is the only permitted
    /// runtime origin besides model download, and it runs on-device. This
    /// isn't enforced here yet: the pipeline wiring (issue #25) is where a
    /// non-local base URL should be rejected at config time rather than
    /// silently reached. `redirects(0)` on [`UreqTransport`] already prevents
    /// a local responder from bouncing the request off-origin at runtime.
    pub fn new(base_url: impl Into<String>, model: impl Into<String>, transport: T) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            transport,
        }
    }

    /// Builds an `OllamaCleanup` against [`DEFAULT_OLLAMA_BASE_URL`].
    pub fn with_default_base_url(model: impl Into<String>, transport: T) -> Self {
        Self::new(DEFAULT_OLLAMA_BASE_URL, model, transport)
    }

    fn clean_via_ollama(&self, raw: &str) -> Result<String, CleanupError> {
        let request = GenerateRequest {
            model: &self.model,
            system: CLEANUP_PROMPT_V1,
            prompt: raw,
            stream: false,
        };
        let body = serde_json::to_string(&request).map_err(|_| CleanupError::Unreachable)?;
        let url = format!("{}/api/generate", self.base_url.trim_end_matches('/'));

        let response_body = self
            .transport
            .post(&url, &body)
            .map_err(|_| CleanupError::Unreachable)?;

        let parsed: GenerateResponse =
            serde_json::from_str(&response_body).map_err(|_| CleanupError::Unreachable)?;

        Ok(parsed.response.trim().to_string())
    }
}

impl<T: OllamaTransport> Cleanup for OllamaCleanup<T> {
    fn clean(&self, raw: &str, tone: Tone) -> Result<String, CleanupError> {
        match tone {
            Tone::Verbatim => Ok(raw.to_string()),
            Tone::Neutral => self.clean_via_ollama(raw),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table-driven cases: (description, raw input, expected cleaned output)
    /// under `Tone::Neutral`, covering each `RegexCleanup` transform in turn.
    const CASES: &[(&str, &str, &str)] = &[
        (
            "removes unambiguous filler interjections (um/uh/er)",
            "Um, I think, uh, this works, er, fine",
            "I think, this works, fine.",
        ),
        (
            "removes like/you know only when comma-flanked (discourse-marker use)",
            "So, like, this is cool, you know, right",
            "So, this is cool, right.",
        ),
        (
            "keeps 'like' when not comma-flanked (comparative use, not a filler)",
            "It looks like rain today",
            "It looks like rain today.",
        ),
        (
            "keeps 'you know' when not comma-flanked (literal use, not a filler)",
            "You know the answer",
            "You know the answer.",
        ),
        (
            "collapses duplicate/irregular whitespace",
            "This  is    fine",
            "This is fine.",
        ),
        (
            "capitalizes a lowercase sentence start",
            "hello world",
            "Hello world.",
        ),
        (
            "capitalizes every sentence start, not just the first",
            "hello there. how are you? i am fine!",
            "Hello there. How are you? I am fine!",
        ),
        (
            "leaves already-clean, already-punctuated input untouched",
            "This already has a period.",
            "This already has a period.",
        ),
        (
            "combines filler removal, spacing, capitalization, and punctuation",
            "um,  i think, like,  this  is, uh, the plan, you know,  right",
            "I think, this is, the plan, right.",
        ),
    ];

    #[test]
    fn regex_cleanup_transforms_neutral_tone() {
        let cleanup = RegexCleanup;
        for (description, raw, expected) in CASES {
            let got = cleanup
                .clean(raw, Tone::Neutral)
                .unwrap_or_else(|e| panic!("{description}: clean() returned Err({e:?})"));
            assert_eq!(&got, expected, "case failed: {description}");
        }
    }

    #[test]
    fn regex_cleanup_is_idempotent_on_already_clean_input() {
        let cleanup = RegexCleanup;
        for (description, _, expected) in CASES {
            let twice = cleanup
                .clean(expected, Tone::Neutral)
                .unwrap_or_else(|e| panic!("{description}: clean() returned Err({e:?})"));
            assert_eq!(&twice, expected, "not idempotent: {description}");
        }
    }

    #[test]
    fn regex_cleanup_empty_input_stays_empty() {
        let cleanup = RegexCleanup;
        assert_eq!(cleanup.clean("", Tone::Neutral).unwrap(), "");
        assert_eq!(cleanup.clean("   ", Tone::Neutral).unwrap(), "");
    }

    #[test]
    fn regex_cleanup_verbatim_tone_bypasses_all_transforms() {
        let cleanup = RegexCleanup;
        let raw = "  um, hello   world, like, this is,uh,messy";
        assert_eq!(cleanup.clean(raw, Tone::Verbatim).unwrap(), raw);
    }

    #[test]
    fn regex_cleanup_keeps_genuine_list_connector_like_issue_52() {
        // Sentinel issue #52: comma-flanked "like" isn't always discourse
        // filler — "eggs, like, milk" uses "like" as a genuine list
        // connector ("such as"), and stripping it produces a nonsensical
        // "Eggs, milk." The word must survive when it isn't followed by a
        // clause (contrast with the CASES table above, where "like," is
        // followed by a clause starter like "this" and is correctly
        // stripped as filler).
        let cleanup = RegexCleanup;
        let got = cleanup.clean("eggs, like, milk", Tone::Neutral).unwrap();
        assert_eq!(got, "Eggs, like, milk.");
    }

    #[test]
    fn regex_cleanup_never_returns_unreachable() {
        // RegexCleanup is the always-available baseline (ADR-0005) — it must
        // never surface the Unreachable variant reserved for the future
        // Ollama-backed implementation (issue #20).
        let cleanup = RegexCleanup;
        for tone in [Tone::Neutral, Tone::Verbatim] {
            assert!(!matches!(
                cleanup.clean("um, test", tone),
                Err(CleanupError::Unreachable)
            ));
        }
    }
}

#[cfg(test)]
mod ollama_tests {
    //! Issue #20 (AC-4, AC-10): `OllamaCleanup`, the injected-transport
    //! seam, and the versioned rewrite-only prompt. All tests here run
    //! against a `StubTransport` — no real network call, no running
    //! Ollama required.
    use super::*;
    use std::cell::RefCell;

    /// Records the last request the transport was asked to send, and
    /// returns a preprogrammed outcome — lets tests assert both the
    /// fallback decision (AC-4) and the exact request shape (AC-10)
    /// without a real socket.
    struct StubTransport {
        response: Result<String, TransportError>,
        captured: RefCell<Option<(String, String)>>,
    }

    impl StubTransport {
        fn unreachable() -> Self {
            Self {
                response: Err(TransportError::ConnectionFailed),
                captured: RefCell::new(None),
            }
        }

        /// A stub whose call times out (hung-but-reachable endpoint) — the
        /// real [`UreqTransport`] surfaces this once its read/connect
        /// timeout fires, so a hung Ollama can't block the sync call
        /// forever (issue #20 🟡, becomes 🔴 once the paste path is wired).
        fn timing_out() -> Self {
            Self {
                response: Err(TransportError::Timeout),
                captured: RefCell::new(None),
            }
        }

        /// A stub that succeeds, echoing back `model_output` inside a
        /// canned Ollama `/api/generate` JSON response body.
        fn succeeding(model_output: &str) -> Self {
            let body = serde_json::json!({ "response": model_output, "done": true }).to_string();
            Self {
                response: Ok(body),
                captured: RefCell::new(None),
            }
        }

        fn captured_request(&self) -> (String, String) {
            self.captured
                .borrow()
                .clone()
                .expect("transport was never called")
        }
    }

    impl OllamaTransport for StubTransport {
        fn post(&self, url: &str, body: &str) -> Result<String, TransportError> {
            *self.captured.borrow_mut() = Some((url.to_string(), body.to_string()));
            self.response.clone()
        }
    }

    fn cleanup_with(transport: StubTransport) -> OllamaCleanup<StubTransport> {
        OllamaCleanup::new("http://localhost:11434", "llama3", transport)
    }

    #[test]
    fn ollama_cleanup_returns_unreachable_when_transport_fails() {
        // AC-4: an unreachable endpoint must surface CleanupError::Unreachable
        // (never a raw transport error, never a panic) so the pipeline can
        // fall back to RegexCleanup.
        let cleanup = cleanup_with(StubTransport::unreachable());
        let result = cleanup.clean("um, hello there", Tone::Neutral);
        assert_eq!(result, Err(CleanupError::Unreachable));
    }

    #[test]
    fn ollama_cleanup_maps_transport_timeout_to_unreachable() {
        // AC-4: a hung-but-reachable endpoint whose call times out must
        // ALSO surface CleanupError::Unreachable (not block forever, not
        // return Ok), so the timeout path still triggers the RegexCleanup
        // fallback and never wedges the paste path.
        let cleanup = cleanup_with(StubTransport::timing_out());
        let result = cleanup.clean("um, hello there", Tone::Neutral);
        assert_eq!(result, Err(CleanupError::Unreachable));
    }

    #[test]
    fn pipeline_style_fallback_surfaces_no_error_when_unreachable() {
        // AC-4, end to end within this module: when OllamaCleanup can't
        // reach the endpoint, dispatching to RegexCleanup on
        // Err(Unreachable) yields a normal Ok result with no error ever
        // reaching a hypothetical paste path.
        let ollama = cleanup_with(StubTransport::unreachable());
        let regex = RegexCleanup;
        let raw = "um, hello world";

        let cleaned = match ollama.clean(raw, Tone::Neutral) {
            Ok(text) => text,
            Err(CleanupError::Unreachable) => regex
                .clean(raw, Tone::Neutral)
                .expect("RegexCleanup is infallible"),
        };

        assert_eq!(cleaned, "Hello world.");
    }

    #[test]
    fn ollama_cleanup_verbatim_tone_bypasses_the_transport_entirely() {
        // Mirrors RegexCleanup's Verbatim contract and, critically, proves
        // Verbatim never touches the network at all.
        let stub = StubTransport::unreachable();
        let cleanup = cleanup_with(stub);
        let raw = "  um, hello   world, uh, messy";
        assert_eq!(cleanup.clean(raw, Tone::Verbatim).unwrap(), raw);
        assert!(cleanup.transport.captured.borrow().is_none());
    }

    #[test]
    fn ollama_cleanup_targets_the_configured_base_url() {
        let stub = StubTransport::succeeding("Hello world.");
        let cleanup = OllamaCleanup::new("http://localhost:11434", "llama3", stub);
        cleanup.clean("hello world", Tone::Neutral).unwrap();
        let (url, _) = cleanup.transport.captured_request();
        assert_eq!(url, "http://localhost:11434/api/generate");
    }

    #[test]
    fn ollama_cleanup_default_base_url_is_localhost_11434() {
        // MISSION §5: the only permitted runtime origin besides model
        // download is localhost:11434 — the default must not point
        // anywhere else.
        assert_eq!(DEFAULT_OLLAMA_BASE_URL, "http://localhost:11434");
    }

    /// AC-10 fixture: self-corrections, missing punctuation, and a spoken
    /// list, all in one transcript.
    const FIXTURE_RAW: &str =
        "so the meeting is at i mean the meeting is tomorrow at 3pm and we need to bring \
         the laptop the charger and the notes";

    /// A well-behaved model response: corrections resolved, punctuation
    /// restored, spoken list rendered as bullets. Used to assert the
    /// pipeline passes a faithful model response through unchanged
    /// (rewrite-only property: this module adds nothing beyond the input).
    const FIXTURE_MODEL_OUTPUT: &str = "The meeting is tomorrow at 3pm. We need to bring:\n- The laptop\n- The charger\n- The notes";

    #[test]
    fn well_behaved_model_response_is_relayed_faithfully_without_added_content() {
        // NOTE: this is a PASS-THROUGH / faithful-relay test, NOT a test of
        // rewrite *behavior*. The corrections, punctuation, and bullets in
        // FIXTURE_MODEL_OUTPUT are produced by the stubbed model, not by any
        // code under test — so this asserts only that OllamaCleanup relays a
        // well-behaved response verbatim (modulo trimming) and introduces no
        // content of its own (the rewrite-only property on the return side).
        // Actual rewrite-quality coverage against a real model is a recorded-
        // Ollama integration follow-up (see PR body).
        let stub = StubTransport::succeeding(FIXTURE_MODEL_OUTPUT);
        let cleanup = cleanup_with(stub);

        let got = cleanup.clean(FIXTURE_RAW, Tone::Neutral).unwrap();

        assert_eq!(
            got, FIXTURE_MODEL_OUTPUT,
            "the model response must be relayed byte-for-byte (after trimming)"
        );
    }

    #[test]
    fn relayed_response_is_trimmed_but_otherwise_untouched() {
        // Guards the one transform OllamaCleanup DOES apply to the response:
        // trimming leading/trailing whitespace. Interior content — including
        // the newlines/bullets of a list — must survive unchanged.
        let stub = StubTransport::succeeding("  \nHello.\n- one\n- two\n  ");
        let cleanup = cleanup_with(stub);
        let got = cleanup.clean("hello one two", Tone::Neutral).unwrap();
        assert_eq!(got, "Hello.\n- one\n- two");
    }

    #[test]
    fn ac10_request_carries_the_rewrite_only_prompt_and_the_raw_input_in_the_correct_fields() {
        // AC-10's rewrite-only property: assert the request sent to the
        // stub carries the rewrite-only prompt plus the untouched input —
        // this module must not editorialize on what it sends upstream
        // either.
        //
        // Deserialize and assert PER FIELD (not substring-over-flattened-
        // JSON): the `system` field must be the prompt and the `prompt`
        // field must be the raw input. A substring check would still pass
        // if the two were swapped (prompt: PROMPT, system: raw), which is a
        // real request-construction regression — this test must catch it.
        let stub = StubTransport::succeeding(FIXTURE_MODEL_OUTPUT);
        let cleanup = cleanup_with(stub);
        cleanup.clean(FIXTURE_RAW, Tone::Neutral).unwrap();

        let (_, body) = cleanup.transport.captured_request();
        let parsed: serde_json::Value =
            serde_json::from_str(&body).expect("request body must be valid JSON");

        assert_eq!(
            parsed["system"], CLEANUP_PROMPT_V1,
            "the `system` field must carry the rewrite-only prompt, not the transcript"
        );
        assert_eq!(
            parsed["prompt"], FIXTURE_RAW,
            "the `prompt` field must carry the raw transcript verbatim, not the prompt"
        );
        assert_eq!(
            parsed["model"], "llama3",
            "the configured model must be sent"
        );
        assert_eq!(
            parsed["stream"], false,
            "streaming must be disabled so the response is a single JSON object"
        );
    }

    #[test]
    fn ac10_prompt_file_contains_the_rewrite_only_constraints() {
        // Regression guard: if a future prompt edit drops one of these
        // constraints, this test fails CI (MISSION §7).
        let prompt = CLEANUP_PROMPT_V1.to_lowercase();
        for must_contain in [
            "never answer",
            "never add",
            "filler",
            "self-correction",
            "punctuation",
            "bullet",
            "tone",
        ] {
            assert!(
                prompt.contains(must_contain),
                "prompt is missing required constraint: {must_contain:?}"
            );
        }
    }
}
