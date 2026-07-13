//! Local persistence via `rusqlite` (+ `tauri-plugin-store` for simple settings).
//!
//! Owns history, personal dictionary, and snippets — all local-only, under the
//! OS app-data dir (MISSION §5: no server, nothing leaves the device).
//!
//! Pure logic over the DB layer should stay unit-testable; the connection/IO
//! boundary is the only OS-adjacent part.
//!
//! Stub — no logic yet; implemented in a later M1 increment.

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------
    // Migrations
    // -------------------------------------------------------------

    #[test]
    fn opening_an_in_memory_store_runs_migration_1_and_sets_user_version_to_1() {
        let store = Store::open_in_memory().expect("open_in_memory should succeed");
        assert_eq!(store.schema_version().unwrap(), 1);
    }

    #[test]
    fn reopening_an_on_disk_store_is_idempotent_and_keeps_user_version_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");

        {
            let store = Store::open(&path).expect("first open should succeed");
            assert_eq!(store.schema_version().unwrap(), 1);
        }

        // Re-opening an already-migrated DB must not error and must leave
        // user_version untouched — the migration runner is forward-only and
        // idempotent.
        let store = Store::open(&path).expect("second open should succeed");
        assert_eq!(store.schema_version().unwrap(), 1);
    }

    // -------------------------------------------------------------
    // insert / search round-trip
    // -------------------------------------------------------------

    #[test]
    fn insert_then_search_round_trips_the_row_contents() {
        let store = Store::open_in_memory().unwrap();

        let id = store
            .insert_history(1_000, "hello world", "Hello, world.", Some("Notes"))
            .expect("insert should succeed");
        assert!(id > 0);

        let results = store.search_history("hello", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, id);
        assert_eq!(results[0].created_at_ms, 1_000);
        assert_eq!(results[0].raw, "hello world");
        assert_eq!(results[0].cleaned, "Hello, world.");
        assert_eq!(results[0].app_name.as_deref(), Some("Notes"));
    }

    #[test]
    fn search_matches_case_insensitively_over_raw_and_cleaned() {
        let store = Store::open_in_memory().unwrap();
        store
            .insert_history(1_000, "the QUICK fox", "quick fox.", None)
            .unwrap();

        let by_raw = store.search_history("quick", 10).unwrap();
        assert_eq!(by_raw.len(), 1);

        let by_cleaned = store.search_history("QUICK", 10).unwrap();
        assert_eq!(by_cleaned.len(), 1);
    }

    #[test]
    fn search_with_no_match_returns_an_empty_vec() {
        let store = Store::open_in_memory().unwrap();
        store.insert_history(1_000, "alpha", "alpha.", None).unwrap();

        let results = store.search_history("zzz-nomatch", 10).unwrap();
        assert_eq!(results, Vec::new());
    }

    // -------------------------------------------------------------
    // LIKE wildcard escaping
    // -------------------------------------------------------------

    #[test]
    fn search_query_containing_percent_matches_only_the_literal_percent() {
        let store = Store::open_in_memory().unwrap();
        store
            .insert_history(1_000, "score: 50% off", "Score: 50% off.", None)
            .unwrap();
        store
            .insert_history(2_000, "score: 50X off", "Score: 50X off.", None)
            .unwrap();

        // A naive LIKE query would treat `%` as a wildcard and match both
        // rows; escaped, `50%` must match only the row with a literal `%`.
        let results = store.search_history("50%", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].raw, "score: 50% off");
    }

    #[test]
    fn search_query_containing_underscore_matches_only_the_literal_underscore() {
        let store = Store::open_in_memory().unwrap();
        store
            .insert_history(1_000, "file_name here", "file_name here.", None)
            .unwrap();
        store
            .insert_history(2_000, "fileXname here", "fileXname here.", None)
            .unwrap();

        // Unescaped, `_` in LIKE matches any single character, which would
        // also match "fileXname". Escaped, it must match only the literal
        // underscore.
        let results = store.search_history("file_name", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].raw, "file_name here");
    }

    // -------------------------------------------------------------
    // search limit + ordering
    // -------------------------------------------------------------

    #[test]
    fn search_results_are_newest_first_and_capped_by_limit() {
        let store = Store::open_in_memory().unwrap();
        for (i, ms) in [1_000, 3_000, 2_000].into_iter().enumerate() {
            store
                .insert_history(ms, &format!("match {i}"), &format!("match {i}."), None)
                .unwrap();
        }

        let results = store.search_history("match", 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].created_at_ms, 3_000);
        assert_eq!(results[1].created_at_ms, 2_000);
    }

    // -------------------------------------------------------------
    // delete / clear
    // -------------------------------------------------------------

    #[test]
    fn delete_history_removes_only_the_targeted_row() {
        let store = Store::open_in_memory().unwrap();
        let keep = store.insert_history(1_000, "keep me", "keep me.", None).unwrap();
        let drop = store.insert_history(2_000, "drop me", "drop me.", None).unwrap();

        store.delete_history(drop).unwrap();

        let results = store.search_history("me", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, keep);
    }

    #[test]
    fn clear_history_removes_every_row() {
        let store = Store::open_in_memory().unwrap();
        store.insert_history(1_000, "one", "one.", None).unwrap();
        store.insert_history(2_000, "two", "two.", None).unwrap();

        store.clear_history().unwrap();

        let results = store.search_history("", 10).unwrap();
        assert_eq!(results, Vec::new());
    }

    // -------------------------------------------------------------
    // prune_history boundary
    // -------------------------------------------------------------

    #[test]
    fn prune_history_deletes_strictly_older_rows_and_keeps_the_row_exactly_at_cutoff() {
        let store = Store::open_in_memory().unwrap();
        let cutoff = 5_000;

        store
            .insert_history(cutoff - 1, "older than cutoff", "x.", None)
            .unwrap();
        let at_cutoff = store
            .insert_history(cutoff, "exactly at cutoff", "x.", None)
            .unwrap();
        let newer = store
            .insert_history(cutoff + 1, "newer than cutoff", "x.", None)
            .unwrap();

        let deleted = store.prune_history(cutoff).unwrap();
        assert_eq!(deleted, 1);

        let remaining = store.search_history("cutoff", 10).unwrap();
        let mut remaining_ids: Vec<i64> = remaining.iter().map(|r| r.id).collect();
        remaining_ids.sort_unstable();
        let mut expected_ids = vec![at_cutoff, newer];
        expected_ids.sort_unstable();
        assert_eq!(remaining_ids, expected_ids);
    }

    // -------------------------------------------------------------
    // retention_cutoff_ms (pure)
    // -------------------------------------------------------------

    #[test]
    fn retention_cutoff_ms_of_zero_days_means_keep_forever() {
        assert_eq!(retention_cutoff_ms(1_000_000, 0), None);
    }

    #[test]
    fn retention_cutoff_ms_of_positive_days_subtracts_the_injected_now() {
        let now_ms: i64 = 10 * 24 * 60 * 60 * 1000; // day 10
        let cutoff = retention_cutoff_ms(now_ms, 3).unwrap();
        assert_eq!(cutoff, 7 * 24 * 60 * 60 * 1000); // day 7
    }
}
