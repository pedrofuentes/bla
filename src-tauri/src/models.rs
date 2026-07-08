//! First-run Whisper model downloader (issue #24, ADR-0004, MISSION §5, PRD
//! AC-12).
//!
//! TDD red step: the spec below exercises the registry, the AC-12 network
//! guard, checksum verification, progress math, resume planning, target-path
//! construction, and the injected-transport download orchestration this
//! module will provide. None of it exists yet — the production types and
//! functions land in the next commit.

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
                spec.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
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
            sink.write_all(remaining).map_err(|e| ModelError::Io(e.to_string()))?;
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
        assert_eq!(std::fs::read(&target).unwrap(), body);
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
        assert!(!transport.called.get(), "transport must not be invoked for a disallowed origin");
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
        assert!(!target.exists(), "target must never be created on checksum mismatch");
        let partial = partial_download_path(dir.path(), &spec);
        assert!(!partial.exists(), "corrupt partial must be removed so a retry starts fresh");
    }

    #[test]
    fn download_model_with_spec_resumes_from_an_existing_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let full_body = b"0123456789ABCDEFGHIJ".to_vec();
        let spec = spec_for(&full_body, "fixture-d.bin");

        // Pre-seed a partial file with the first half of the bytes.
        let partial_path = partial_download_path(dir.path(), &spec);
        std::fs::create_dir_all(partial_path.parent().unwrap()).unwrap();
        std::fs::write(&partial_path, &full_body[..10]).unwrap();

        // The fake transport only ever serves the FULL body sliced from the
        // requested offset onward, mirroring a real ranged response.
        let transport = FakeTransport::new(full_body.clone());
        let target = download_model_with_spec(&transport, &spec, dir.path(), |_| {})
            .expect("resumed download should succeed");

        assert_eq!(std::fs::read(&target).unwrap(), full_body);
    }

    #[test]
    fn download_model_with_spec_skips_the_transport_when_already_complete_and_checksum_holds() {
        let dir = tempfile::tempdir().unwrap();
        let body = b"already fully downloaded bytes".to_vec();
        let spec = spec_for(&body, "fixture-e.bin");

        let partial_path = partial_download_path(dir.path(), &spec);
        std::fs::create_dir_all(partial_path.parent().unwrap()).unwrap();
        std::fs::write(&partial_path, &body).unwrap();

        let transport = FakeTransport::new(Vec::new()); // must never be called
        let target = download_model_with_spec(&transport, &spec, dir.path(), |_| {})
            .expect("already-complete download should succeed without the network");

        assert!(!transport.called.get());
        assert_eq!(std::fs::read(&target).unwrap(), body);
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
