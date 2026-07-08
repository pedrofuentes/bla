//! First-run Whisper model downloader (issue #24, ADR-0004, MISSION §5, PRD
//! AC-12).
//!
//! Two supported presets (ADR-0004, PRD AC-17): quantized **`large-v3-turbo`
//! (q5_0)**, the default, and **`small`**, the fast/low-RAM option. Both are
//! GGUF/`ggml` files published at the well-known `ggerganov/whisper.cpp`
//! Hugging Face repo. [`model_registry`] pins each preset's file name,
//! download URL, and expected SHA-256 checksum (taken from that repo's
//! Git-LFS metadata, `lfs.oid`, at the time this module was written); the
//! target path lives under the caller-supplied OS app-data dir, never in the
//! repo (`.gitignore` excludes `*.gguf`/`*.bin`/`models/` — this module never
//! writes inside the repo tree, only the app-data dir a caller passes in).
//!
//! **AC-12 network guard — the tested logic.** [`download_url`] is a pure
//! function from [`ModelPreset`] to its registry URL; [`is_allowlisted_url`]
//! is a pure predicate enforcing MISSION §5's model-download allowlist
//! (`huggingface.co` and its CDN — including the newer `hf.co`-hosted Xet
//! storage backend HF's CDN redirects resolve to, e.g.
//! `us.aws.cdn.hf.co`/`cas-bridge.xethub.hf.co`). Crucially it parses the URL
//! with the **same `url` crate `ureq` resolves the connect target with**, so
//! the host the guard checks can never diverge from the host actually dialed
//! — closing the parser-differential bypass class (`https://evil.com?@huggingface.co`
//! and friends, where a hand-rolled authority scan would read `huggingface.co`
//! but the real host is `evil.com`), alongside lookalike hosts
//! (`huggingface.co.evil.com`) and the userinfo trick
//! (`https://huggingface.co@evil.com/`). A test asserts every
//! [`model_registry`] URL passes this guard, and a battery of adversarial
//! cases assert the guard rejects everything it should. [`follow_redirects`]
//! (used by [`UreqTransport`]) re-checks every redirect hop against this same
//! guard at the real network boundary — not just the request's initial
//! origin — so the runtime egress invariant holds even if a redirect were to
//! point somewhere unexpected.
//!
//! **Download orchestration is thin, TDD-exempt glue** (AGENTS.md
//! OS-integration exemption): the actual HTTP GET, streaming-to-disk, and
//! progress reporting live behind the injected [`ModelTransport`] trait
//! ([`UreqTransport`] is the real, `ureq`-backed implementation), so
//! [`download_model_with_spec`] — URL/allowlist selection, resume-vs-restart
//! planning ([`plan_resume`]), progress-percent math ([`compute_progress`]),
//! target-path construction ([`model_target_path`]), and checksum
//! verification ([`verify_checksum`]) — is exercised in tests against a fake
//! in-memory transport, with no real socket or downloaded model file
//! involved. A checksum mismatch is always an error: the partial file is
//! removed and the target is never created, so a corrupt or incomplete
//! model can never be mistaken for a ready one.
//!
//! **Progress** is reported as a [`DownloadProgress`] value (bytes done,
//! total bytes, computed percent) via a plain callback; wiring that callback
//! to a Tauri event the UI subscribes to is glue left to the eventual
//! `commands.rs` integration (not this module — this module has no Tauri
//! `AppHandle` dependency, matching `cleanup`'s OS-integration boundary).
//!
//! **Privacy (MISSION §5):** the only origin this module's transport will
//! ever contact is `huggingface.co` and its CDN, enforced at both the
//! registry (test-covered) and the transport (redirect re-check) layers.
//! Errors here never carry model bytes or file contents, only file paths
//! and status codes/messages.
//!
//! `mod models` isn't wired into `commands.rs` yet — that lands with the
//! first-run UI integration (issue #24's UI half). Until then this file's
//! items are only reachable from its own unit tests, so `dead_code` is
//! silenced here rather than crate-wide (mirrors `cleanup.rs`/`output.rs`).
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The Hugging Face repo every supported preset is published from (ADR-0004:
/// "a well-known HF GGUF source").
pub const HF_REPO: &str = "ggerganov/whisper.cpp";

/// A supported Whisper model preset (ADR-0004, PRD AC-17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelPreset {
    /// Quantized `large-v3-turbo` (q5_0) — the default: ≈ real-time on
    /// Apple Silicon within the AC-2 latency budget.
    LargeV3TurboQ5,
    /// `small` — the fast/low-RAM option.
    Small,
}

impl ModelPreset {
    /// Every supported preset, for exhaustive iteration (used by the AC-12
    /// network-guard test to cover the whole registry, and by the future
    /// M2 model picker, AC-17).
    pub const ALL: [ModelPreset; 2] = [ModelPreset::LargeV3TurboQ5, ModelPreset::Small];

    /// A stable, settings-safe identifier for this preset (independent of
    /// the underlying file name, which is an HF implementation detail).
    pub fn id(self) -> &'static str {
        match self {
            ModelPreset::LargeV3TurboQ5 => "large-v3-turbo-q5",
            ModelPreset::Small => "small",
        }
    }
}

/// Everything the downloader needs to know about one preset: which file to
/// fetch, from where, its expected exact size and SHA-256, and the file name
/// it's stored under in the app-data model dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    pub preset: ModelPreset,
    /// File name as published by HF — also the file name used under the
    /// app-data model dir (see [`model_target_path`]).
    pub filename: &'static str,
    /// Full download URL. Always `https://huggingface.co/...` — see
    /// [`download_url`] and the AC-12 network guard.
    pub url: &'static str,
    /// Lower-case hex SHA-256 of the file, from HF's Git-LFS metadata
    /// (`lfs.oid`) for this exact file. [`verify_checksum`] must pass
    /// against this before the download is considered ready.
    pub sha256: &'static str,
    /// Exact file size in bytes, from HF's file metadata. Used to plan
    /// resume-vs-restart ([`plan_resume`]) and to size progress before the
    /// server responds.
    pub size_bytes: u64,
}

/// The registry of supported presets (ADR-0004, PRD AC-17). Source of truth
/// for [`download_url`] and every other preset-derived value in this module.
fn registry(preset: ModelPreset) -> ModelSpec {
    match preset {
        ModelPreset::LargeV3TurboQ5 => ModelSpec {
            preset,
            filename: "ggml-large-v3-turbo-q5_0.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
            sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
            size_bytes: 574_041_195,
        },
        ModelPreset::Small => ModelSpec {
            preset,
            filename: "ggml-small.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
            sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
            size_bytes: 487_601_967,
        },
    }
}

/// [`registry`] for every preset, as a fixed array — handy for tests and any
/// future caller that needs the whole registry (e.g. the M2 model picker).
pub fn model_registry() -> [ModelSpec; 2] {
    [
        registry(ModelPreset::LargeV3TurboQ5),
        registry(ModelPreset::Small),
    ]
}

/// Pure function from a preset to its download URL (AC-12's tested seam).
/// Always resolves to `https://huggingface.co/...` — see
/// [`is_allowlisted_url`], which every URL this function can return must
/// pass (asserted exhaustively in this module's tests).
pub fn download_url(preset: ModelPreset) -> &'static str {
    registry(preset).url
}

/// Target path for `spec`'s model file under `app_data_dir` (ADR-0004: the
/// OS app-data dir, never the repo). Pure — takes the base dir and spec as
/// parameters rather than resolving either itself, so it's unit-testable
/// without a Tauri `AppHandle` and without the real (multi-hundred-MB)
/// registry entries.
pub fn model_target_path(app_data_dir: &Path, spec: &ModelSpec) -> PathBuf {
    app_data_dir.join("models").join(spec.filename)
}

/// Path for `spec`'s in-progress download, alongside its final target (same
/// directory, `.partial` suffix). Kept distinct from [`model_target_path`]
/// so a partial/corrupt download is never mistaken for a ready model — only
/// a checksum-verified rename promotes one to the other (see
/// [`download_model_with_spec`]).
pub fn partial_download_path(app_data_dir: &Path, spec: &ModelSpec) -> PathBuf {
    let mut name = spec.filename.to_string();
    name.push_str(".partial");
    app_data_dir.join("models").join(name)
}

// ---------------------------------------------------------------------
// AC-12 network guard
// ---------------------------------------------------------------------

/// Host suffixes allowlisted for model downloads (MISSION §5: allowlists
/// "huggingface.co and its CDN"). `huggingface.co` itself is checked
/// separately (exact match); these are the additional origins its CDN
/// redirects resolve to, including the newer Xet-storage backend observed
/// to redirect to `hf.co`-hosted hosts (e.g. `us.aws.cdn.hf.co`,
/// `cas-bridge.xethub.hf.co`) alongside the classic `cdn-lfs*.huggingface.co`
/// hosts.
const ALLOWED_HOST_SUFFIXES: [&str; 2] = [".huggingface.co", ".hf.co"];
const ALLOWED_EXACT_HOSTS: [&str; 2] = ["huggingface.co", "hf.co"];

/// True if `host` (already lower-cased) is `huggingface.co`/`hf.co` or a
/// subdomain of either — the MISSION §5 model-download allowlist. Dot-anchored
/// suffix matching, so a lookalike like `evilhuggingface.co` (which merely
/// *ends with* the substring `huggingface.co`, sharing no dot boundary) is
/// correctly rejected.
pub fn is_allowlisted_host(host: &str) -> bool {
    ALLOWED_EXACT_HOSTS.contains(&host)
        || ALLOWED_HOST_SUFFIXES
            .iter()
            .any(|suffix| host.ends_with(suffix))
}

/// The AC-12 network guard: true only if `url` is an `https://` URL whose
/// host is allowlisted per [`is_allowlisted_host`]. Every URL
/// [`download_url`] can return must pass this (asserted in this module's
/// tests); [`UreqTransport`] also re-applies it to every redirect hop it
/// follows, so the invariant holds at the real network boundary too, not
/// just at the registry.
///
/// **The URL is parsed with the `url` crate — the SAME parser `ureq`
/// resolves its connect target with** — precisely so the host this guard
/// checks can never diverge from the host that's actually dialed. A
/// hand-rolled authority scan is not safe here: WHATWG URL parsing treats
/// `?` and `#` as authority terminators, so e.g. `https://evil.com?@huggingface.co`
/// has host `evil.com` (everything after `?` is the query), while a naive
/// "take the segment after the last `@`" scan would wrongly read
/// `huggingface.co` and wave a redirect to `evil.com` straight through.
/// Delegating to `url::Url` closes that whole class of parser-differential
/// bypass (`?@`, `#@`, backslash-authority, userinfo, ...).
pub fn is_allowlisted_url(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(parsed) => {
            parsed.scheme() == "https"
                && parsed
                    .host_str()
                    .map(|host| is_allowlisted_host(&host.to_ascii_lowercase()))
                    .unwrap_or(false)
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------
// Checksum verification
// ---------------------------------------------------------------------

/// Errors from downloading, verifying, or planning a model download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// A URL (the registry entry itself, or a redirect `Location`) failed
    /// the AC-12 network guard. Carries the rejected URL for diagnostics —
    /// never model bytes or file contents (MISSION §5: never log anything
    /// sensitive).
    DisallowedOrigin(String),
    /// The downloaded file's SHA-256 didn't match the registry's expected
    /// value. A corrupt or incomplete model is never marked ready.
    ChecksumMismatch { expected: String, actual: String },
    /// A filesystem error while planning, writing, verifying, or promoting
    /// the download.
    Io(String),
    /// The transport failed to complete the request (connection failure,
    /// non-2xx status other than a followable redirect, too many redirects,
    /// ...).
    Transport(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::DisallowedOrigin(url) => {
                write!(f, "origin not allowlisted for model download: {url}")
            }
            ModelError::ChecksumMismatch { expected, actual } => {
                write!(f, "checksum mismatch: expected {expected}, got {actual}")
            }
            ModelError::Io(msg) => write!(f, "model download I/O error: {msg}"),
            ModelError::Transport(msg) => write!(f, "model download transport error: {msg}"),
        }
    }
}

impl std::error::Error for ModelError {}

fn io_err(e: io::Error) -> ModelError {
    ModelError::Io(e.to_string())
}

/// Lower-case hex SHA-256 of `bytes` — pure, deterministic.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Lower-case hex SHA-256 of everything read from `reader`, streamed in
/// fixed-size chunks rather than loaded into memory at once. Generic over
/// `Read` so it's unit-testable against an in-memory buffer as well as a
/// real file.
pub fn sha256_hex_reader<R: Read>(reader: &mut R) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Checks `actual_sha256` (case-insensitively) against `expected_sha256`,
/// the invariant every downloaded model must satisfy before it's promoted
/// to its final target path (a checksum mismatch is always an error — a
/// corrupt/incomplete model is never used).
pub fn verify_checksum(expected_sha256: &str, actual_sha256: &str) -> Result<(), ModelError> {
    if expected_sha256.eq_ignore_ascii_case(actual_sha256) {
        Ok(())
    } else {
        Err(ModelError::ChecksumMismatch {
            expected: expected_sha256.to_string(),
            actual: actual_sha256.to_string(),
        })
    }
}

// ---------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------

/// Download progress, computed purely from a byte count — the value emitted
/// to the UI (as a Tauri event; that emission is glue, not this struct).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    /// `0.0..=100.0`.
    pub percent: f64,
}

/// Computes [`DownloadProgress`] from a byte count. `total_bytes == 0` (size
/// not yet known, e.g. before the server responds) yields `0.0` percent
/// rather than dividing by zero; `bytes_downloaded > total_bytes` (the last
/// chunk landing exactly on/over the boundary) clamps to `100.0` rather than
/// overshooting.
pub fn compute_progress(bytes_downloaded: u64, total_bytes: u64) -> DownloadProgress {
    let percent = if total_bytes == 0 {
        0.0
    } else {
        (bytes_downloaded as f64 / total_bytes as f64 * 100.0).clamp(0.0, 100.0)
    };
    DownloadProgress {
        bytes_downloaded,
        total_bytes,
        percent,
    }
}

/// Throttles progress emission so a subscriber isn't flooded with one
/// callback per 64 KB chunk (issue #24 Sentinel 🟡#4 — a multi-hundred-MB
/// model is thousands of chunks). Emits at most once per whole-percent
/// change of the total, and the caller always forces one final emit on
/// completion. Pure and deterministic (no clock): the decision depends only
/// on byte counts, so it's unit-testable.
#[derive(Debug, Default)]
pub struct ProgressThrottle {
    last_emitted_bucket: Option<u64>,
}

impl ProgressThrottle {
    /// Number of buckets progress is quantized into for throttling — 100 =
    /// whole-percent granularity, so at most ~101 intermediate emits over a
    /// whole download regardless of chunk count.
    const BUCKETS: u64 = 100;

    pub fn new() -> Self {
        Self::default()
    }

    /// Whether progress at `bytes_downloaded`/`total_bytes` should be emitted
    /// now. `is_final` forces an emit (the completed download always fires,
    /// even if it lands in the same percent bucket as the previous emit).
    /// Before the total is known (`total_bytes == 0`) nothing intermediate is
    /// emitted.
    pub fn should_emit(&mut self, bytes_downloaded: u64, total_bytes: u64, is_final: bool) -> bool {
        if is_final {
            self.last_emitted_bucket = Some(Self::BUCKETS);
            return true;
        }
        if total_bytes == 0 {
            return false;
        }
        let bucket = bytes_downloaded.saturating_mul(Self::BUCKETS) / total_bytes;
        if self.last_emitted_bucket == Some(bucket) {
            false
        } else {
            self.last_emitted_bucket = Some(bucket);
            true
        }
    }
}

// ---------------------------------------------------------------------
// Resume / restart planning
// ---------------------------------------------------------------------

/// What to do about an existing `.partial` file before (re)starting a
/// download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumePlan {
    /// No usable partial file — download from byte 0.
    StartFresh,
    /// A partial file smaller than the expected total exists — resume from
    /// this byte offset (a `Range: bytes={0}-` request).
    Resume(u64),
    /// A partial file exactly the expected total size exists — skip the
    /// network fetch entirely and go straight to checksum verification.
    AlreadyComplete,
}

/// Pure resume/restart decision (no I/O — `existing_partial_bytes` is
/// whatever the caller already read from the filesystem).
///
/// - No existing partial (`None`) or an empty one (`Some(0)`): [`ResumePlan::StartFresh`].
/// - Smaller than `expected_total_bytes`: [`ResumePlan::Resume`] from that offset.
/// - Exactly `expected_total_bytes`: [`ResumePlan::AlreadyComplete`] (still
///   subject to checksum verification before being trusted).
/// - Larger than `expected_total_bytes` (a stale/corrupt partial that
///   somehow overshot): [`ResumePlan::StartFresh`] — discard and restart
///   rather than trust an impossible size.
pub fn plan_resume(existing_partial_bytes: Option<u64>, expected_total_bytes: u64) -> ResumePlan {
    match existing_partial_bytes {
        None => ResumePlan::StartFresh,
        Some(0) => ResumePlan::StartFresh,
        Some(n) if n < expected_total_bytes => ResumePlan::Resume(n),
        Some(n) if n == expected_total_bytes => ResumePlan::AlreadyComplete,
        Some(_) => ResumePlan::StartFresh,
    }
}

/// What the transport must do with the existing partial bytes once the
/// server's response status to a (possibly ranged) request is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeDisposition {
    /// Keep the partial bytes and append the response body onto them.
    Append,
    /// Discard the partial bytes (truncate the sink) and write the response
    /// body from offset 0.
    Restart,
}

/// Pure decision (issue #24 Sentinel 🟡#3): given how many bytes we asked to
/// resume from and the status the server actually answered with, decide
/// whether the response body appends onto the partial or replaces it.
///
/// - A fresh download (`requested_resume_bytes == 0`) always [`Append`s](ResumeDisposition::Append)
///   (there's nothing to reconcile — the sink was already truncated).
/// - A resume request (`> 0`) [`Append`s](ResumeDisposition::Append) **only**
///   on HTTP `206 Partial Content`, i.e. the server honored the `Range`.
/// - Any other status to a resume request — most importantly `200 OK`, where
///   the server ignored `Range` and is sending the **whole** file —
///   [`Restart`s](ResumeDisposition::Restart). Appending a full 200 body onto
///   an existing partial would corrupt the file and over-report the total.
pub fn resume_disposition(requested_resume_bytes: u64, status: u16) -> ResumeDisposition {
    if requested_resume_bytes == 0 || status == 206 {
        ResumeDisposition::Append
    } else {
        ResumeDisposition::Restart
    }
}

// ---------------------------------------------------------------------
// Injected transport seam + real ureq-backed implementation
// ---------------------------------------------------------------------

/// The destination a [`ModelTransport`] streams a download into. Two ops
/// only: [`append`](DownloadSink::append) bytes, or [`restart`](DownloadSink::restart)
/// (discard everything written so far and rewind to byte 0). `restart` is
/// what makes the 🟡#3 fix safe: when a resume request is answered with a
/// full `200` body instead of `206`, the transport truncates the partial
/// rather than appending full-onto-partial. The file-backed impl is thin OS
/// glue ([`FileSink`]); tests use an in-memory `Vec` sink so the append /
/// restart / partial-file behavior is exercised without touching disk.
pub trait DownloadSink {
    fn append(&mut self, buf: &[u8]) -> Result<(), ModelError>;
    /// Discard everything written so far and rewind to byte 0.
    fn restart(&mut self) -> Result<(), ModelError>;
}

/// File-backed [`DownloadSink`] — thin OS glue over a `.partial` file handle.
pub struct FileSink {
    file: File,
}

impl FileSink {
    pub fn new(file: File) -> Self {
        Self { file }
    }
}

impl DownloadSink for FileSink {
    fn append(&mut self, buf: &[u8]) -> Result<(), ModelError> {
        // Seek to EOF before writing so resumed appends still land after the
        // existing partial bytes. We deliberately open the handle read+write
        // rather than append-only (see the `.partial` open site): on Windows
        // an append-only handle lacks FILE_WRITE_DATA, so `restart`'s
        // `set_len(0)` would be denied (os error 5). Managing the position
        // explicitly here keeps `restart` truncation valid on every platform.
        self.file.seek(SeekFrom::End(0)).map_err(io_err)?;
        self.file.write_all(buf).map_err(io_err)
    }

    fn restart(&mut self) -> Result<(), ModelError> {
        self.file.set_len(0).map_err(io_err)?;
        self.file.seek(SeekFrom::Start(0)).map_err(io_err)?;
        Ok(())
    }
}

/// Injected HTTP transport seam (mirrors `cleanup.rs`'s `OllamaTransport`).
/// All of [`download_model_with_spec`]'s decision-making — URL/allowlist
/// selection, resume planning, progress math, checksum verification — is
/// pure and tested against a fake implementation of this trait; only
/// [`UreqTransport`] touches a real socket.
pub trait ModelTransport {
    /// Fetches `url`, optionally resuming from `resume_from_bytes` (`0` =
    /// from scratch), streaming the response body into `sink` and invoking
    /// `on_chunk(total_bytes_on_disk, server_reported_total)` after every
    /// chunk written. If a resume request is not honored (see
    /// [`resume_disposition`]) the transport must [`restart`](DownloadSink::restart)
    /// the sink and write the full body from 0. Returns the final byte count
    /// on disk. `server_reported_total` is `None` until the response is
    /// available, then the full file size (a resumed request's total, not
    /// just the remaining-bytes count).
    fn fetch(
        &self,
        url: &str,
        resume_from_bytes: u64,
        sink: &mut dyn DownloadSink,
        on_chunk: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<u64, ModelError>;
}

/// Maximum redirect hops the redirect loop ([`follow_redirects`]) will follow
/// before giving up.
pub const MAX_REDIRECTS: u8 = 5;

/// One HTTP request's outcome as the redirect loop's per-hop responder
/// reports it (issue #24 Sentinel 🟡#6). Generic over the terminal-body type
/// `B` so [`follow_redirects`] stays pure and testable: the real transport
/// uses `B = ureq::Response`, tests use a canned fake so the per-hop
/// allowlist re-check, redirect cap, and missing-`Location` handling are
/// covered with no socket.
pub enum Hop<B> {
    /// A 3xx redirect carrying its raw `Location` header (`None` if the
    /// server omitted it).
    Redirect(Option<String>),
    /// A terminal (non-3xx) response ready for the caller to stream.
    Terminal(B),
}

/// Pure redirect-following loop with the AC-12 allowlist re-checked on
/// **every** hop (issue #24 Sentinel 🟡#6 + the 🔴 blocker's runtime half):
/// the start URL and every `Location` a redirect points at must pass
/// [`is_allowlisted_url`] before it's dialed, so a redirect can never bounce
/// the download off the MISSION §5 allowlist. Enforces [`MAX_REDIRECTS`] and
/// rejects a redirect with no `Location`. `request` performs one hop (a real
/// socket call in [`UreqTransport`]; a canned outcome in tests).
pub fn follow_redirects<B, F>(start_url: &str, mut request: F) -> Result<B, ModelError>
where
    F: FnMut(&str) -> Result<Hop<B>, ModelError>,
{
    if !is_allowlisted_url(start_url) {
        return Err(ModelError::DisallowedOrigin(start_url.to_string()));
    }
    let mut current = start_url.to_string();
    let mut redirects: u8 = 0;
    loop {
        match request(&current)? {
            Hop::Terminal(body) => return Ok(body),
            Hop::Redirect(location) => {
                let location = location.ok_or_else(|| {
                    ModelError::Transport("redirect response missing Location header".into())
                })?;
                if !is_allowlisted_url(&location) {
                    return Err(ModelError::DisallowedOrigin(location));
                }
                redirects += 1;
                if redirects > MAX_REDIRECTS {
                    return Err(ModelError::Transport("too many redirects".to_string()));
                }
                current = location;
            }
        }
    }
}

/// Default connect timeout for [`UreqTransport`] (issue #24 Sentinel 🟡#2):
/// how long to wait to establish the TCP/TLS connection before giving up, so
/// a black-hole host can't hang the first-run download indefinitely.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default per-read timeout for [`UreqTransport`] (issue #24 Sentinel 🟡#2):
/// how long to wait for the next chunk of body once connected. Bounded so a
/// connection that stalls mid-stream fails instead of hanging forever;
/// generous enough not to trip on a slow-but-live CDN.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(60);

/// The real transport: a synchronous `ureq` GET, built with `redirects(0)`
/// so redirects are driven through [`follow_redirects`] — every hop, not
/// just the first request, is re-checked against [`is_allowlisted_url`]. It
/// sets explicit connect/read timeouts so a black-hole or stalled host
/// fails instead of hanging (🟡#2), and honors [`resume_disposition`] so a
/// `200` answer to a `Range` request restarts the sink instead of appending
/// a full body onto the partial (🟡#3). This is the only code in the module
/// that opens a real socket, and it refuses to send a single byte anywhere
/// off the MISSION §5 allowlist, including a redirect target.
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        Self::with_timeouts(DEFAULT_CONNECT_TIMEOUT, DEFAULT_READ_TIMEOUT)
    }

    pub fn with_timeouts(connect_timeout: Duration, read_timeout: Duration) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .redirects(0)
                .timeout_connect(connect_timeout)
                .timeout_read(read_timeout)
                .build(),
        }
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelTransport for UreqTransport {
    fn fetch(
        &self,
        url: &str,
        resume_from_bytes: u64,
        sink: &mut dyn DownloadSink,
        on_chunk: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<u64, ModelError> {
        let response = follow_redirects(url, |current| {
            let mut req = self.agent.get(current);
            if resume_from_bytes > 0 {
                req = req.set("Range", &format!("bytes={resume_from_bytes}-"));
            }
            let resp = req
                .call()
                .map_err(|e| ModelError::Transport(e.to_string()))?;
            if (300..400).contains(&resp.status()) {
                Ok(Hop::Redirect(resp.header("Location").map(str::to_string)))
            } else {
                Ok(Hop::Terminal(resp))
            }
        })?;

        // 🟡#3: only append onto the partial when the server honored the
        // Range (206). A 200 (Range ignored — full body) restarts the sink.
        let base = match resume_disposition(resume_from_bytes, response.status()) {
            ResumeDisposition::Append => resume_from_bytes,
            ResumeDisposition::Restart => {
                sink.restart()?;
                0
            }
        };

        let total_bytes = response
            .header("Content-Length")
            .and_then(|s| s.parse::<u64>().ok())
            .map(|body_len| body_len + base);

        let mut reader = response.into_reader();
        let mut buf = [0u8; 64 * 1024];
        let mut written = base;
        loop {
            let n = reader.read(&mut buf).map_err(io_err)?;
            if n == 0 {
                break;
            }
            sink.append(&buf[..n])?;
            written += n as u64;
            on_chunk(written, total_bytes);
        }
        Ok(written)
    }
}

// ---------------------------------------------------------------------
// Orchestration (thin glue over the pure logic above)
// ---------------------------------------------------------------------

/// Downloads (or resumes) `spec` into `app_data_dir`, verifies its checksum,
/// and only then promotes it to its final [`model_target_path`]. Composes
/// the pure logic above:
///
/// 1. Reject `spec.url` up front if it fails [`is_allowlisted_url`]
///    (defense in depth — the registry already guarantees this).
/// 2. [`plan_resume`] against any existing `.partial` file.
/// 3. Unless [`ResumePlan::AlreadyComplete`], call `transport.fetch` and
///    forward progress through [`compute_progress`] to `on_progress`.
/// 4. [`sha256_hex_reader`] the `.partial` file and [`verify_checksum`] it
///    against `spec.sha256`. On mismatch, delete the `.partial` file (so a
///    retry starts fresh) and return `Err`; the target path is never
///    created or overwritten.
/// 5. On success, rename `.partial` to the final target and return its path.
///
/// This function itself is thin orchestration glue (TDD-exempt), but is
/// exercised in this module's tests against a fake in-memory
/// [`ModelTransport`] and a synthetic [`ModelSpec`] (not the real,
/// multi-hundred-megabyte registry entries) — no real network or disk-sized
/// download needed to cover the composition above end to end.
pub fn download_model_with_spec<T: ModelTransport>(
    transport: &T,
    spec: &ModelSpec,
    app_data_dir: &Path,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<PathBuf, ModelError> {
    if !is_allowlisted_url(spec.url) {
        return Err(ModelError::DisallowedOrigin(spec.url.to_string()));
    }

    let target = model_target_path(app_data_dir, spec);
    let partial = partial_download_path(app_data_dir, spec);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }

    let existing_bytes = fs::metadata(&partial).ok().map(|m| m.len());
    let plan = plan_resume(existing_bytes, spec.size_bytes);

    match plan {
        ResumePlan::AlreadyComplete => {
            on_progress(compute_progress(spec.size_bytes, spec.size_bytes));
        }
        ResumePlan::StartFresh | ResumePlan::Resume(_) => {
            let from_byte = if let ResumePlan::Resume(n) = plan {
                n
            } else {
                0
            };
            let file = if from_byte > 0 {
                // Resume: keep existing bytes, append after them. The
                // transport may still `restart` this handle (truncate) if the
                // server ignores the Range and sends a full 200 body (🟡#3).
                // Open read+write (NOT append-only): FileSink::append seeks to
                // EOF before writing, and a plain write handle lets `restart`'s
                // set_len(0) truncate on Windows too (an append-only handle
                // there lacks FILE_WRITE_DATA → set_len denied, os error 5).
                OpenOptions::new()
                    .create(true)
                    .read(true)
                    .write(true)
                    .truncate(false)
                    .open(&partial)
            } else {
                OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&partial)
            }
            .map_err(io_err)?;
            let mut sink = FileSink::new(file);

            // 🟡#4: throttle intermediate progress so the subscriber isn't
            // flooded with a callback per 64 KB chunk.
            let written = {
                let mut throttle = ProgressThrottle::new();
                let mut emit = |done: u64, total: Option<u64>| {
                    let total = total.unwrap_or(spec.size_bytes);
                    if throttle.should_emit(done, total, false) {
                        on_progress(compute_progress(done, total));
                    }
                };
                transport.fetch(spec.url, from_byte, &mut sink, &mut emit)?
            };
            // Always fire one final progress on completion (🟡#4).
            on_progress(compute_progress(written, spec.size_bytes));
        }
    }

    let actual_sha256 = {
        let mut file = File::open(&partial).map_err(io_err)?;
        sha256_hex_reader(&mut file).map_err(io_err)?
    };

    if let Err(err) = verify_checksum(spec.sha256, &actual_sha256) {
        let _ = fs::remove_file(&partial);
        return Err(err);
    }

    fs::rename(&partial, &target).map_err(io_err)?;
    Ok(target)
}

/// [`download_model_with_spec`] against a registry [`ModelPreset`] rather
/// than a caller-supplied [`ModelSpec`] — the entry point production code
/// (the eventual first-run downloader UI wiring) calls.
pub fn download_model<T: ModelTransport>(
    transport: &T,
    preset: ModelPreset,
    app_data_dir: &Path,
    on_progress: impl FnMut(DownloadProgress),
) -> Result<PathBuf, ModelError> {
    let spec = registry(preset);
    download_model_with_spec(transport, &spec, app_data_dir, on_progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------
    // AC-12: registry URLs + the network-guard predicate
    // -------------------------------------------------------------

    #[test]
    fn download_url_resolves_to_the_expected_huggingface_resolve_path() {
        assert_eq!(
            download_url(ModelPreset::LargeV3TurboQ5),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin"
        );
        assert_eq!(
            download_url(ModelPreset::Small),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
        );
    }

    #[test]
    fn every_registry_entry_has_a_64_char_lowercase_hex_sha256() {
        for spec in model_registry() {
            assert_eq!(
                spec.sha256.len(),
                64,
                "{}: sha256 must be 64 hex chars",
                spec.filename
            );
            assert!(
                spec.sha256
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{}: sha256 must be lowercase hex",
                spec.filename
            );
        }
    }

    #[test]
    fn ac12_every_registry_url_passes_the_network_guard() {
        // The core AC-12 assertion: this FAILS if any preset's download URL
        // resolves outside the allowlisted origins.
        for preset in ModelPreset::ALL {
            let url = download_url(preset);
            assert!(
                is_allowlisted_url(url),
                "preset {preset:?} has a non-allowlisted download URL: {url}"
            );
        }
    }

    #[test]
    fn allowlist_accepts_huggingface_co_and_its_cdn() {
        for url in [
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
            "https://cdn-lfs.huggingface.co/repos/abc/def",
            "https://cdn-lfs-us-1.huggingface.co/repos/abc/def",
            "https://hf.co/some/path",
            // Real CDN redirect targets observed for this repo's files
            // (the newer Xet-storage backend HF migrated to):
            "https://us.aws.cdn.hf.co/xet-bridge-us/abc/def",
            "https://cas-bridge.xethub.hf.co/abc",
        ] {
            assert!(is_allowlisted_url(url), "should be allowlisted: {url}");
        }
    }

    #[test]
    fn allowlist_rejects_non_https_schemes() {
        for url in [
            "http://huggingface.co/foo",
            "ftp://huggingface.co/foo",
            "huggingface.co/foo",
        ] {
            assert!(!is_allowlisted_url(url), "should be rejected: {url}");
        }
    }

    #[test]
    fn allowlist_rejects_lookalike_and_subdomain_confusion_hosts() {
        for url in [
            // Shares the substring "huggingface.co" but not a dot boundary.
            "https://evilhuggingface.co/foo",
            "https://notreallyhuggingface.co/foo",
            // Subdomain-confusion: huggingface.co as a *subdomain* of evil.com.
            "https://huggingface.co.evil.com/foo",
            "https://cdn-lfs.huggingface.co.evil.com/foo",
            // Path/query lookalikes, not the actual host.
            "https://evil.com/https://huggingface.co/",
            "https://evil.com/?huggingface.co",
            // Wrong domain entirely.
            "https://example.com/ggml-small.bin",
        ] {
            assert!(!is_allowlisted_url(url), "should be rejected: {url}");
        }
    }

    #[test]
    fn allowlist_rejects_the_userinfo_phishing_trick() {
        // Classic address-bar trick: everything before the LAST unescaped
        // '@' is userinfo, not the host — the real host here is evil.com.
        assert!(!is_allowlisted_url(
            "https://huggingface.co@evil.com/ggml-small.bin"
        ));
        assert!(!is_allowlisted_url(
            "https://user:pass@huggingface.co.evil.com/foo"
        ));
    }

    #[test]
    fn allowlist_handles_ports_and_malformed_urls_without_panicking() {
        assert!(is_allowlisted_url("https://huggingface.co:443/foo"));
        assert!(!is_allowlisted_url("https://evil.com:443/foo"));
        assert!(!is_allowlisted_url(""));
        assert!(!is_allowlisted_url("https://"));
        assert!(!is_allowlisted_url("not a url at all"));
    }

    #[test]
    fn allowlist_rejects_parser_differential_authority_bypasses() {
        // 🔴 REGRESSION (Sentinel PR #78): these have NO `/` before the `?`
        // or `#`, so a hand-rolled "host = segment after the last `@`" scan
        // read `huggingface.co` and waved them through — while the `url`
        // crate `ureq` actually connects with resolves the host to
        // `evil.com`. That divergence is the whole allowlist bypass. Because
        // the guard now uses the SAME parser, host == evil.com == rejected.
        for url in [
            "https://evil.com?@huggingface.co",
            "https://evil.com#@huggingface.co",
            "https://evil.com?x=@huggingface.co/ggml-small.bin",
            "https://evil.com#@huggingface.co/ggml-small.bin",
            // Backslash-authority variant: WHATWG treats `\` like `/`, so the
            // authority ends at the backslash and the host is `evil.com`.
            "https://evil.com\\@huggingface.co",
            "https://evil.com\\.huggingface.co/foo",
        ] {
            assert!(
                !is_allowlisted_url(url),
                "parser-differential bypass must be rejected: {url}"
            );
        }
    }

    #[test]
    fn allowlist_still_accepts_legitimate_query_and_fragment_on_allowlisted_hosts() {
        // Guard the flip side of the bypass fix: a real allowlisted host with
        // a query or fragment (HF CDN URLs carry signed query strings) must
        // still pass.
        assert!(is_allowlisted_url(
            "https://us.aws.cdn.hf.co/xet-bridge-us/abc?X-Amz-Signature=deadbeef&Expires=123"
        ));
        assert!(is_allowlisted_url(
            "https://huggingface.co/a/b?ref=main#frag"
        ));
    }

    // -------------------------------------------------------------
    // Checksum
    // -------------------------------------------------------------

    #[test]
    fn sha256_hex_matches_known_test_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_hex_reader_matches_sha256_hex_over_the_same_bytes() {
        let data = b"the quick brown fox jumps over the lazy dog".repeat(100);
        let mut cursor = std::io::Cursor::new(&data);
        assert_eq!(sha256_hex_reader(&mut cursor).unwrap(), sha256_hex(&data));
    }

    #[test]
    fn verify_checksum_accepts_a_case_insensitive_match() {
        assert!(verify_checksum(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        )
        .is_ok());
    }

    #[test]
    fn verify_checksum_rejects_a_mismatch() {
        let err = verify_checksum("abc123", "def456").unwrap_err();
        assert_eq!(
            err,
            ModelError::ChecksumMismatch {
                expected: "abc123".to_string(),
                actual: "def456".to_string(),
            }
        );
    }

    // -------------------------------------------------------------
    // Progress
    // -------------------------------------------------------------

    #[test]
    fn compute_progress_reports_expected_percentages() {
        assert_eq!(compute_progress(0, 100).percent, 0.0);
        assert_eq!(compute_progress(50, 100).percent, 50.0);
        assert_eq!(compute_progress(100, 100).percent, 100.0);
    }

    #[test]
    fn compute_progress_zero_total_is_zero_percent_not_a_panic() {
        let p = compute_progress(0, 0);
        assert_eq!(p.percent, 0.0);
        let p = compute_progress(5, 0);
        assert_eq!(p.percent, 0.0);
    }

    #[test]
    fn compute_progress_clamps_overshoot_to_100() {
        let p = compute_progress(150, 100);
        assert_eq!(p.percent, 100.0);
    }

    #[test]
    fn compute_progress_carries_the_raw_byte_counts_through() {
        let p = compute_progress(42, 1000);
        assert_eq!(p.bytes_downloaded, 42);
        assert_eq!(p.total_bytes, 1000);
    }

    // -------------------------------------------------------------
    // Resume / restart planning
    // -------------------------------------------------------------

    #[test]
    fn plan_resume_no_partial_starts_fresh() {
        assert_eq!(plan_resume(None, 100), ResumePlan::StartFresh);
    }

    #[test]
    fn plan_resume_empty_partial_starts_fresh() {
        assert_eq!(plan_resume(Some(0), 100), ResumePlan::StartFresh);
    }

    #[test]
    fn plan_resume_smaller_partial_resumes_from_its_offset() {
        assert_eq!(plan_resume(Some(40), 100), ResumePlan::Resume(40));
    }

    #[test]
    fn plan_resume_exact_size_partial_is_already_complete() {
        assert_eq!(plan_resume(Some(100), 100), ResumePlan::AlreadyComplete);
    }

    #[test]
    fn plan_resume_oversized_partial_discards_and_restarts() {
        assert_eq!(plan_resume(Some(150), 100), ResumePlan::StartFresh);
    }

    // -------------------------------------------------------------
    // Target paths
    // -------------------------------------------------------------

    #[test]
    fn model_target_path_is_under_a_models_subdir_of_app_data() {
        let base = Path::new("/app-data");
        assert_eq!(
            model_target_path(base, &registry(ModelPreset::LargeV3TurboQ5)),
            PathBuf::from("/app-data/models/ggml-large-v3-turbo-q5_0.bin")
        );
        assert_eq!(
            model_target_path(base, &registry(ModelPreset::Small)),
            PathBuf::from("/app-data/models/ggml-small.bin")
        );
    }

    #[test]
    fn model_target_path_resolves_correctly_under_a_windows_style_app_data_base_issue_98() {
        // `r"C:\Users\x\AppData\Roaming\bla"` is just a Rust string literal —
        // it compiles and asserts identically on every host this repo
        // builds on (including this macOS dev machine, which never sees a
        // real `C:\` path from its own OS). `model_target_path` only ever
        // appends (`Path::join`), never parses or splits `base`, so a
        // Windows-shaped base round-trips through it intact regardless of
        // which platform is running the test — that's the seam this test
        // locks in.
        let windows_base = PathBuf::from(r"C:\Users\x\AppData\Roaming\bla");
        let spec = registry(ModelPreset::Small);

        let target = model_target_path(&windows_base, &spec);

        assert!(
            target.starts_with(&windows_base),
            "target must live under the supplied app-data base, got {target:?}"
        );
        assert_eq!(target.file_name().unwrap(), "ggml-small.bin");
        assert_eq!(target, windows_base.join("models").join("ggml-small.bin"));
    }

    #[test]
    fn partial_download_path_differs_from_the_final_target_and_is_stable() {
        let base = Path::new("/app-data");
        for preset in ModelPreset::ALL {
            let spec = registry(preset);
            let target = model_target_path(base, &spec);
            let partial = partial_download_path(base, &spec);
            assert_ne!(target, partial);
            assert_eq!(partial, {
                let mut p = target.clone();
                let mut name = p.file_name().unwrap().to_os_string();
                name.push(".partial");
                p.set_file_name(name);
                p
            });
        }
    }

    // -------------------------------------------------------------
    // Orchestration, against a fake in-memory transport
    // -------------------------------------------------------------

    /// An in-memory [`DownloadSink`] for transport-level tests — records the
    /// full byte stream and honors `restart` (truncate) exactly like the
    /// real file-backed sink.
    #[derive(Default)]
    struct VecSink {
        bytes: Vec<u8>,
    }

    impl DownloadSink for VecSink {
        fn append(&mut self, buf: &[u8]) -> Result<(), ModelError> {
            self.bytes.extend_from_slice(buf);
            Ok(())
        }
        fn restart(&mut self) -> Result<(), ModelError> {
            self.bytes.clear();
            Ok(())
        }
    }

    /// A fake [`ModelTransport`] that serves fixed bytes from memory and
    /// records how it was called — no real socket, ever. `honor_range`
    /// models whether the simulated server honors a `Range` request: when
    /// `true` it appends only the bytes past `resume_from_bytes` (a `206`);
    /// when `false` it `restart`s the sink and serves the full body from 0
    /// (a `200` that ignored the range — the 🟡#3 case). `last_resume_from`
    /// records the offset it was actually asked to resume from (🟡#5), so a
    /// test can prove the resume offset propagated rather than silently
    /// defaulting to 0.
    struct FakeTransport {
        body: Vec<u8>,
        honor_range: bool,
        called: std::cell::Cell<bool>,
        last_resume_from: std::cell::Cell<Option<u64>>,
    }

    impl FakeTransport {
        fn new(body: impl Into<Vec<u8>>) -> Self {
            Self {
                body: body.into(),
                honor_range: true,
                called: std::cell::Cell::new(false),
                last_resume_from: std::cell::Cell::new(None),
            }
        }

        /// A fake whose simulated server ignores `Range` and always sends the
        /// full body with a `200` (exercises the 🟡#3 restart path).
        fn ignoring_range(body: impl Into<Vec<u8>>) -> Self {
            Self {
                honor_range: false,
                ..Self::new(body)
            }
        }
    }

    impl ModelTransport for FakeTransport {
        fn fetch(
            &self,
            _url: &str,
            resume_from_bytes: u64,
            sink: &mut dyn DownloadSink,
            on_chunk: &mut dyn FnMut(u64, Option<u64>),
        ) -> Result<u64, ModelError> {
            self.called.set(true);
            self.last_resume_from.set(Some(resume_from_bytes));
            let total = self.body.len() as u64;
            let base = if resume_from_bytes > 0 && self.honor_range {
                resume_from_bytes
            } else {
                // Fresh, or a server that ignored the Range (200): the sink
                // starts (or is truncated back to) empty and gets the whole
                // body.
                if resume_from_bytes > 0 {
                    sink.restart()?;
                }
                0
            };
            let slice = &self.body[(base as usize).min(self.body.len())..];
            sink.append(slice)?;
            let written = base + slice.len() as u64;
            on_chunk(written, Some(total));
            Ok(written)
        }
    }

    fn spec_for(body: &[u8], filename: &'static str) -> ModelSpec {
        ModelSpec {
            preset: ModelPreset::Small,
            filename,
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/test-fixture.bin",
            sha256: Box::leak(sha256_hex(body).into_boxed_str()),
            size_bytes: body.len() as u64,
        }
    }

    #[test]
    fn download_model_with_spec_succeeds_and_verifies_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let body = b"a synthetic model payload, not a real weights file".to_vec();
        let spec = spec_for(&body, "fixture-a.bin");
        let transport = FakeTransport::new(body.clone());

        let mut last_progress = None;
        let result = download_model_with_spec(&transport, &spec, dir.path(), |p| {
            last_progress = Some(p);
        });

        let target = result.expect("download should succeed");
        assert_eq!(target, model_target_path(dir.path(), &spec));
        assert_eq!(fs::read(&target).unwrap(), body);
        assert!(!partial_download_path(dir.path(), &spec).exists());
        let progress = last_progress.expect("on_progress should have been called");
        assert_eq!(progress.percent, 100.0);
    }

    #[test]
    fn download_model_with_spec_rejects_a_disallowed_origin_without_calling_the_transport() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = spec_for(b"irrelevant", "fixture-b.bin");
        spec.url = "https://evil.com/ggml-small.bin";
        let transport = FakeTransport::new(b"irrelevant".to_vec());

        let err = download_model_with_spec(&transport, &spec, dir.path(), |_| {}).unwrap_err();

        assert!(matches!(err, ModelError::DisallowedOrigin(_)));
        assert!(
            !transport.called.get(),
            "transport must not be invoked for a disallowed origin"
        );
    }

    #[test]
    fn download_model_with_spec_removes_the_partial_file_and_errors_on_checksum_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let body = b"the actual bytes served".to_vec();
        let mut spec = spec_for(&body, "fixture-c.bin");
        spec.sha256 = Box::leak("0".repeat(64).into_boxed_str()); // guaranteed wrong
        let transport = FakeTransport::new(body);

        let err = download_model_with_spec(&transport, &spec, dir.path(), |_| {}).unwrap_err();

        assert!(matches!(err, ModelError::ChecksumMismatch { .. }));
        let target = model_target_path(dir.path(), &spec);
        assert!(
            !target.exists(),
            "target must never be created on checksum mismatch"
        );
        let partial = partial_download_path(dir.path(), &spec);
        assert!(
            !partial.exists(),
            "corrupt partial must be removed so a retry starts fresh"
        );
    }

    #[test]
    fn download_model_with_spec_resumes_from_an_existing_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let full_body = b"0123456789ABCDEFGHIJ".to_vec();
        let spec = spec_for(&full_body, "fixture-d.bin");

        // Pre-seed a partial file with the first half of the bytes.
        let partial_path = partial_download_path(dir.path(), &spec);
        fs::create_dir_all(partial_path.parent().unwrap()).unwrap();
        fs::write(&partial_path, &full_body[..10]).unwrap();

        // The fake transport serves a 206-style ranged response: only the
        // bytes past the requested offset.
        let transport = FakeTransport::new(full_body.clone());
        let target = download_model_with_spec(&transport, &spec, dir.path(), |_| {})
            .expect("resumed download should succeed");

        assert_eq!(fs::read(&target).unwrap(), full_body);
        // 🟡#5: the transport must actually have been asked to resume from
        // the partial's length — not silently restart from 0. This assertion
        // goes RED if the orchestration passes the wrong offset (e.g. if
        // ResumePlan::Resume were mishandled as StartFresh).
        assert_eq!(
            transport.last_resume_from.get(),
            Some(10),
            "transport must be asked to resume from the existing partial's byte length"
        );
    }

    #[test]
    fn download_model_with_spec_restarts_when_server_ignores_the_range_and_sends_full_body() {
        // 🟡#3: a resume request answered with a full 200 body (Range
        // ignored) must truncate the partial and rewrite from scratch —
        // never append full-onto-partial (which would corrupt the file and
        // fail the checksum).
        let dir = tempfile::tempdir().unwrap();
        let full_body = b"0123456789ABCDEFGHIJ".to_vec();
        let spec = spec_for(&full_body, "fixture-d2.bin");

        // Pre-seed a partial with a DIFFERENT first 10 bytes than the real
        // file, so appending-onto-partial would demonstrably corrupt it.
        let partial_path = partial_download_path(dir.path(), &spec);
        fs::create_dir_all(partial_path.parent().unwrap()).unwrap();
        fs::write(&partial_path, b"XXXXXXXXXX").unwrap();

        let transport = FakeTransport::ignoring_range(full_body.clone());
        let target = download_model_with_spec(&transport, &spec, dir.path(), |_| {})
            .expect("restart-on-200 download should succeed");

        assert_eq!(
            fs::read(&target).unwrap(),
            full_body,
            "the corrupt partial must have been discarded, not appended onto"
        );
        assert_eq!(transport.last_resume_from.get(), Some(10));
    }

    #[test]
    fn download_model_with_spec_skips_the_transport_when_already_complete_and_checksum_holds() {
        let dir = tempfile::tempdir().unwrap();
        let body = b"already fully downloaded bytes".to_vec();
        let spec = spec_for(&body, "fixture-e.bin");

        let partial_path = partial_download_path(dir.path(), &spec);
        fs::create_dir_all(partial_path.parent().unwrap()).unwrap();
        fs::write(&partial_path, &body).unwrap();

        let transport = FakeTransport::new(Vec::new()); // must never be called
        let target = download_model_with_spec(&transport, &spec, dir.path(), |_| {})
            .expect("already-complete download should succeed without the network");

        assert!(!transport.called.get());
        assert_eq!(fs::read(&target).unwrap(), body);
    }

    #[test]
    fn download_model_delegates_to_download_model_with_spec_using_the_registry() {
        // Smoke-tests the registry-backed entry point end to end against
        // the real Small preset's *shape* (URL/filename) but a fake
        // transport standing in for the real multi-hundred-MB payload —
        // this intentionally does NOT verify the real checksum (that would
        // require the actual model bytes); it only proves the wiring calls
        // through with the registry's spec.
        let dir = tempfile::tempdir().unwrap();
        let real_spec = registry(ModelPreset::Small);
        let body = b"stand-in bytes, not the real model".to_vec();
        let transport = FakeTransport::new(body);

        let err = download_model(&transport, ModelPreset::Small, dir.path(), |_| {}).unwrap_err();
        // Expected to fail checksum verification (fake bytes vs. the real
        // registry's checksum) — proves the real registry sha256 is what
        // was checked against, and that the URL used matched the registry.
        assert!(matches!(err, ModelError::ChecksumMismatch { .. }));
        assert_eq!(real_spec.url, download_url(ModelPreset::Small));
    }

    // -------------------------------------------------------------
    // 🟡#3: resume disposition (206 appends, anything else restarts)
    // -------------------------------------------------------------

    #[test]
    fn resume_disposition_appends_on_206_and_restarts_otherwise() {
        // Fresh download: always append (nothing to reconcile), whatever the
        // status.
        assert_eq!(resume_disposition(0, 200), ResumeDisposition::Append);
        assert_eq!(resume_disposition(0, 206), ResumeDisposition::Append);
        // Resume request honored → append.
        assert_eq!(resume_disposition(100, 206), ResumeDisposition::Append);
        // Resume request answered with a full 200 (Range ignored) → restart.
        assert_eq!(resume_disposition(100, 200), ResumeDisposition::Restart);
        // Any other status to a resume request also restarts (defensive).
        assert_eq!(resume_disposition(100, 416), ResumeDisposition::Restart);
    }

    // -------------------------------------------------------------
    // 🟡#4: progress throttling
    // -------------------------------------------------------------

    #[test]
    fn progress_throttle_emits_at_most_once_per_percent_plus_the_final() {
        // Simulate a large download delivered in many small (64 KB) chunks:
        // without throttling this would be ~15k callbacks; throttled it must
        // be at most ~101 intermediate + 1 final.
        let total: u64 = 1_000_000_000;
        let chunk: u64 = 64 * 1024;
        let mut throttle = ProgressThrottle::new();
        let mut emitted = 0usize;
        let mut done = 0u64;
        while done < total {
            done = (done + chunk).min(total);
            if throttle.should_emit(done, total, false) {
                emitted += 1;
            }
        }
        // Final emit is always forced.
        assert!(throttle.should_emit(total, total, true));
        assert!(
            emitted <= 101,
            "throttled intermediate emits should be ≤101, got {emitted}"
        );
        assert!(
            emitted >= 90,
            "should still emit steady progress, got {emitted}"
        );
    }

    #[test]
    fn progress_throttle_is_quiet_before_the_total_is_known() {
        let mut throttle = ProgressThrottle::new();
        assert!(!throttle.should_emit(123, 0, false));
        // But a final emit still fires even with an unknown total.
        assert!(throttle.should_emit(123, 0, true));
    }

    // -------------------------------------------------------------
    // 🟡#6: the redirect / per-hop allowlist re-check loop
    // -------------------------------------------------------------

    #[test]
    fn follow_redirects_returns_the_terminal_body_on_a_direct_hit() {
        let result: Result<&str, _> =
            follow_redirects("https://huggingface.co/ggml-small.bin", |_url| {
                Ok(Hop::Terminal("body"))
            });
        assert_eq!(result.unwrap(), "body");
    }

    #[test]
    fn follow_redirects_follows_allowlisted_hops_to_the_terminal_body() {
        let hops = std::cell::Cell::new(0u8);
        let result: Result<&str, _> =
            follow_redirects("https://huggingface.co/ggml-small.bin", |_url| {
                let n = hops.get();
                hops.set(n + 1);
                match n {
                    0 => Ok(Hop::Redirect(Some(
                        "https://cdn-lfs.huggingface.co/a".to_string(),
                    ))),
                    1 => Ok(Hop::Redirect(Some(
                        "https://us.aws.cdn.hf.co/b".to_string(),
                    ))),
                    _ => Ok(Hop::Terminal("cdn-body")),
                }
            });
        assert_eq!(result.unwrap(), "cdn-body");
    }

    #[test]
    fn follow_redirects_rejects_a_redirect_to_a_disallowed_host() {
        // Deleting the per-hop guard in follow_redirects makes this test go
        // RED — the whole point of 🟡#6's coverage.
        let result: Result<&str, _> =
            follow_redirects("https://huggingface.co/ggml-small.bin", |_url| {
                Ok(Hop::Redirect(Some("https://evil.com/steal".to_string())))
            });
        assert!(
            matches!(result, Err(ModelError::DisallowedOrigin(u)) if u == "https://evil.com/steal")
        );
    }

    #[test]
    fn follow_redirects_rejects_the_parser_differential_bypass_at_the_redirect_layer() {
        // The 🔴 bypass, but arriving as a redirect Location rather than the
        // initial URL: it must be rejected here too.
        let result: Result<&str, _> =
            follow_redirects("https://huggingface.co/ggml-small.bin", |_url| {
                Ok(Hop::Redirect(Some(
                    "https://evil.com?@huggingface.co".to_string(),
                )))
            });
        assert!(matches!(result, Err(ModelError::DisallowedOrigin(_))));
    }

    #[test]
    fn follow_redirects_rejects_a_redirect_without_a_location_header() {
        let result: Result<&str, _> =
            follow_redirects("https://huggingface.co/ggml-small.bin", |_url| {
                Ok(Hop::Redirect(None))
            });
        assert!(matches!(result, Err(ModelError::Transport(_))));
    }

    #[test]
    fn follow_redirects_gives_up_after_too_many_redirects() {
        // A responder that never terminates — always redirects to an
        // allowlisted host — must be stopped by the MAX_REDIRECTS cap rather
        // than looping forever.
        let result: Result<&str, _> = follow_redirects("https://huggingface.co/start", |_url| {
            Ok(Hop::Redirect(Some(
                "https://huggingface.co/again".to_string(),
            )))
        });
        assert!(
            matches!(result, Err(ModelError::Transport(m)) if m.contains("too many redirects"))
        );
    }

    #[test]
    fn follow_redirects_rejects_a_disallowed_start_url_without_calling_the_responder() {
        let called = std::cell::Cell::new(false);
        let result: Result<&str, _> = follow_redirects("https://evil.com/start", |_url| {
            called.set(true);
            Ok(Hop::Terminal("nope"))
        });
        assert!(matches!(result, Err(ModelError::DisallowedOrigin(_))));
        assert!(
            !called.get(),
            "responder must not run for a disallowed start URL"
        );
    }

    // -------------------------------------------------------------
    // DownloadSink (the restart seam behind the 🟡#3 fix)
    // -------------------------------------------------------------

    #[test]
    fn vec_sink_append_then_restart_discards_prior_bytes() {
        let mut sink = VecSink::default();
        sink.append(b"partial-garbage").unwrap();
        sink.restart().unwrap();
        sink.append(b"clean").unwrap();
        assert_eq!(sink.bytes, b"clean");
    }
}
