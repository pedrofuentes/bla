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
//! `us.aws.cdn.hf.co`/`cas-bridge.xethub.hf.co`) with real host-matching
//! (dot-boundary suffix checks, userinfo stripped, `https` required) rather
//! than a naive substring check, so it isn't defeated by a lookalike host
//! (`huggingface.co.evil.com`) or a userinfo trick
//! (`https://huggingface.co@evil.com/`). A test asserts every
//! [`model_registry`] URL passes this guard, and a battery of adversarial
//! cases assert the guard rejects everything it should. [`UreqTransport`]
//! (below) additionally re-checks every redirect hop against this same guard
//! at the real network boundary — not just the request's initial origin —
//! so the runtime egress invariant holds even if a redirect were to point
//! somewhere unexpected.
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
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

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

/// Extracts the host from an `https://` URL, stripping any userinfo
/// (`user@`) and port, and rejecting every other scheme. Manual (not a full
/// URI parser) because this module only ever needs to answer "is this host
/// allowlisted" for URLs it either built itself or received as a redirect
/// `Location` — but it still resists the classic userinfo phishing trick
/// (`https://huggingface.co@evil.com/`) by taking the authority segment
/// *after* the last unescaped `@`.
fn extract_https_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let authority = match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };
    let host = match authority.rfind(':') {
        Some(i)
            if !authority[i + 1..].is_empty()
                && authority[i + 1..].bytes().all(|b| b.is_ascii_digit()) =>
        {
            &authority[..i]
        }
        _ => authority,
    };
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// The AC-12 network guard: true only if `url` is an `https://` URL whose
/// host is allowlisted per [`is_allowlisted_host`]. Every URL
/// [`download_url`] can return must pass this (asserted in this module's
/// tests); [`UreqTransport`] also re-applies it to every redirect hop it
/// follows, so the invariant holds at the real network boundary too, not
/// just at the registry.
pub fn is_allowlisted_url(url: &str) -> bool {
    match extract_https_host(url) {
        Some(host) => is_allowlisted_host(&host),
        None => false,
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

// ---------------------------------------------------------------------
// Injected transport seam + real ureq-backed implementation
// ---------------------------------------------------------------------

/// Injected HTTP transport seam (mirrors `cleanup.rs`'s `OllamaTransport`).
/// All of [`download_model_with_spec`]'s decision-making — URL/allowlist
/// selection, resume planning, progress math, checksum verification — is
/// pure and tested against a fake implementation of this trait; only
/// [`UreqTransport`] touches a real socket.
pub trait ModelTransport {
    /// Fetches `url`, optionally resuming from `resume_from_bytes` (`0` =
    /// from scratch), streaming the response body into `sink` and invoking
    /// `on_chunk(total_bytes_written_including_resume_offset, server_reported_total)`
    /// after every chunk written. `server_reported_total` is `None` until
    /// the server's response is available, then the full file size (a
    /// resumed request's total, not just the remaining-bytes count).
    fn fetch(
        &self,
        url: &str,
        resume_from_bytes: u64,
        sink: &mut dyn Write,
        on_chunk: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), ModelError>;
}

/// Maximum redirect hops [`UreqTransport`] will follow before giving up.
pub const MAX_REDIRECTS: u8 = 5;

/// The real transport: a synchronous `ureq` GET, built with `redirects(0)`
/// and a manual redirect loop so every hop — not just the initial request —
/// is re-checked against [`is_allowlisted_url`] before being followed. This
/// is the only code in the module that opens a real socket, and it refuses
/// to send a single byte anywhere off the MISSION §5 allowlist, including a
/// redirect target.
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new().redirects(0).build(),
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
        sink: &mut dyn Write,
        on_chunk: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), ModelError> {
        if !is_allowlisted_url(url) {
            return Err(ModelError::DisallowedOrigin(url.to_string()));
        }

        let mut current_url = url.to_string();
        let mut hops = 0u8;
        let response = loop {
            let mut req = self.agent.get(&current_url);
            if resume_from_bytes > 0 {
                req = req.set("Range", &format!("bytes={resume_from_bytes}-"));
            }
            let resp = req
                .call()
                .map_err(|e| ModelError::Transport(e.to_string()))?;

            if (300..400).contains(&resp.status()) {
                hops += 1;
                if hops > MAX_REDIRECTS {
                    return Err(ModelError::Transport("too many redirects".to_string()));
                }
                let location = resp
                    .header("Location")
                    .ok_or_else(|| {
                        ModelError::Transport("redirect response missing Location header".into())
                    })?
                    .to_string();
                if !is_allowlisted_url(&location) {
                    return Err(ModelError::DisallowedOrigin(location));
                }
                current_url = location;
                continue;
            }
            break resp;
        };

        let total_bytes = response
            .header("Content-Length")
            .and_then(|s| s.parse::<u64>().ok())
            .map(|remaining| remaining + resume_from_bytes);

        let mut reader = response.into_reader();
        let mut buf = [0u8; 64 * 1024];
        let mut written = resume_from_bytes;
        loop {
            let n = reader.read(&mut buf).map_err(io_err)?;
            if n == 0 {
                break;
            }
            sink.write_all(&buf[..n]).map_err(io_err)?;
            written += n as u64;
            on_chunk(written, total_bytes);
        }
        Ok(())
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
        ResumePlan::StartFresh => {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&partial)
                .map_err(io_err)?;
            transport.fetch(spec.url, 0, &mut file, &mut |done, total| {
                on_progress(compute_progress(done, total.unwrap_or(spec.size_bytes)));
            })?;
        }
        ResumePlan::Resume(from_byte) => {
            let mut file = OpenOptions::new()
                .append(true)
                .open(&partial)
                .map_err(io_err)?;
            transport.fetch(spec.url, from_byte, &mut file, &mut |done, total| {
                on_progress(compute_progress(done, total.unwrap_or(spec.size_bytes)));
            })?;
        }
        ResumePlan::AlreadyComplete => {
            on_progress(compute_progress(spec.size_bytes, spec.size_bytes));
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

    /// A fake [`ModelTransport`] that serves fixed bytes from memory and
    /// records whether/how it was called — no real socket, ever.
    struct FakeTransport {
        body: Vec<u8>,
        called: std::cell::Cell<bool>,
    }

    impl FakeTransport {
        fn new(body: impl Into<Vec<u8>>) -> Self {
            Self {
                body: body.into(),
                called: std::cell::Cell::new(false),
            }
        }
    }

    impl ModelTransport for FakeTransport {
        fn fetch(
            &self,
            _url: &str,
            resume_from_bytes: u64,
            sink: &mut dyn Write,
            on_chunk: &mut dyn FnMut(u64, Option<u64>),
        ) -> Result<(), ModelError> {
            self.called.set(true);
            let total = self.body.len() as u64;
            let remaining = &self.body[(resume_from_bytes as usize).min(self.body.len())..];
            sink.write_all(remaining)
                .map_err(|e| ModelError::Io(e.to_string()))?;
            on_chunk(total, Some(total));
            Ok(())
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

        // The fake transport only ever serves the FULL body sliced from the
        // requested offset onward, mirroring a real ranged response.
        let transport = FakeTransport::new(full_body.clone());
        let target = download_model_with_spec(&transport, &spec, dir.path(), |_| {})
            .expect("resumed download should succeed");

        assert_eq!(fs::read(&target).unwrap(), full_body);
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
}
