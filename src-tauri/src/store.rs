//! Local persistence via `rusqlite` (+ `tauri-plugin-store` for simple settings).
//!
//! Owns history, personal dictionary, and snippets — all local-only, under the
//! OS app-data dir (MISSION §5: no server, nothing leaves the device).
//!
//! Pure logic over the DB layer should stay unit-testable; the connection/IO
//! boundary is the only OS-adjacent part.
//!
//! M3 PR 3.1 (kickoff #160): headless [`Store`] foundation — a numbered
//! `PRAGMA user_version` migration runner (migration 1 creates `history`)
//! and the history CRUD/search/retention operations. No Tauri wiring, no
//! commands, no frontend — those land in later M3 PRs; this module has no
//! `tauri::` imports so every decision here stays unit-testable against a
//! real in-memory SQLite connection ([`Store::open_in_memory`]), no fakes
//! needed.
//!
//! Privacy (MISSION §5/§7): `raw`/`cleaned` text is sanctioned local SQLite
//! storage, but it is never logged — no `Debug`/`log!`/`println!` of row
//! contents anywhere in this module or its call sites. [`HistoryRow`]
//! derives `Debug` only because tests assert on it; nothing here prints one.

use rusqlite::{params, Connection, Result as SqliteResult};
use std::path::Path;

/// One forward-only migration: the `user_version` it brings the schema to,
/// and the SQL that gets it there. Idempotent by construction — the runner
/// only ever applies a migration whose version is greater than the DB's
/// current `user_version`, so re-opening an already-migrated DB is a no-op.
///
/// Later M3 PRs (dictionary, tone_rules, snippets) add migrations 2/3/4 by
/// appending entries here — nothing about the runner itself needs to change.
type Migration = (i64, &'static str);

const MIGRATIONS: &[Migration] = &[(
    1,
    "CREATE TABLE IF NOT EXISTS history (
        id INTEGER PRIMARY KEY,
        created_at_ms INTEGER NOT NULL,
        raw TEXT NOT NULL,
        cleaned TEXT NOT NULL,
        app_name TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_history_created_at_ms ON history(created_at_ms);",
)];

/// A single dictation history entry (issue #160).
///
/// Derives `Debug` for test assertions only — no code path in this crate
/// logs/prints a `HistoryRow` (MISSION §5/§7: raw/cleaned dictation text
/// must never be logged, even though storing it in local SQLite is
/// sanctioned).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRow {
    pub id: i64,
    pub created_at_ms: i64,
    pub raw: String,
    pub cleaned: String,
    pub app_name: Option<String>,
}

/// Headless SQLite persistence layer wrapping a single [`rusqlite::Connection`].
///
/// No `tauri::` imports anywhere in this module — [`Store::open`] takes a
/// plain [`Path`] (a later PR resolves that path from Tauri's app-data dir
/// and wires commands on top), and [`Store::open_in_memory`] is the headless
/// test double tests use directly (real in-memory SQLite, not a fake).
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if absent) the SQLite database at `path` and run any
    /// pending migrations.
    pub fn open(path: impl AsRef<Path>) -> SqliteResult<Self> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// Open a private, in-memory SQLite database and run migrations. The
    /// headless test double — no filesystem, no fakes, real SQLite.
    pub fn open_in_memory() -> SqliteResult<Self> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> SqliteResult<Self> {
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// The DB's current `PRAGMA user_version` — the schema version tests
    /// assert against to confirm migrations ran (and, on reopen, didn't
    /// re-run).
    #[cfg(test)]
    fn schema_version(&self) -> SqliteResult<i64> {
        self.conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
    }

    /// Apply every migration in [`MIGRATIONS`] whose version is greater
    /// than the DB's current `user_version`, in order, each inside its own
    /// transaction. Forward-only and idempotent: re-running against an
    /// already-migrated DB applies nothing and errors on nothing.
    fn migrate(&mut self) -> SqliteResult<()> {
        let current: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;

        for (version, sql) in MIGRATIONS {
            if *version <= current {
                continue;
            }
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            // PRAGMA doesn't support bound parameters; `version` is a
            // compile-time constant from MIGRATIONS, never user input, so
            // formatting it into the statement is safe.
            tx.execute_batch(&format!("PRAGMA user_version = {version};"))?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Insert a new history row and return its assigned row id.
    pub fn insert_history(
        &self,
        created_at_ms: i64,
        raw: &str,
        cleaned: &str,
        app_name: Option<&str>,
    ) -> SqliteResult<i64> {
        self.conn.execute(
            "INSERT INTO history (created_at_ms, raw, cleaned, app_name) VALUES (?1, ?2, ?3, ?4)",
            params![created_at_ms, raw, cleaned, app_name],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Case-insensitive substring search over `raw` and `cleaned`, newest
    /// first, capped at `limit` rows. Kept behind this one method so a
    /// future FTS5-backed implementation is a drop-in replacement with no
    /// caller changes.
    ///
    /// `query`'s LIKE wildcards (`%`, `_`) are escaped before being sent to
    /// SQLite, so a literal `%` or `_` in the user's search text matches
    /// only that literal character rather than acting as a wildcard.
    pub fn search_history(&self, query: &str, limit: usize) -> SqliteResult<Vec<HistoryRow>> {
        let pattern = format!("%{}%", escape_like(query));
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at_ms, raw, cleaned, app_name FROM history
             WHERE raw LIKE ?1 ESCAPE '\\' OR cleaned LIKE ?1 ESCAPE '\\'
             ORDER BY created_at_ms DESC, id DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit as i64], |row| {
            Ok(HistoryRow {
                id: row.get(0)?,
                created_at_ms: row.get(1)?,
                raw: row.get(2)?,
                cleaned: row.get(3)?,
                app_name: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Delete a single history row by id. Deleting an id that doesn't exist
    /// is a no-op, not an error.
    pub fn delete_history(&self, id: i64) -> SqliteResult<()> {
        self.conn
            .execute("DELETE FROM history WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Delete every history row.
    pub fn clear_history(&self) -> SqliteResult<()> {
        self.conn.execute("DELETE FROM history", [])?;
        Ok(())
    }

    /// Delete every history row strictly older than `cutoff_ms` (a row
    /// exactly at `cutoff_ms` is kept) and return the number of rows
    /// deleted. Pair with [`retention_cutoff_ms`] to compute `cutoff_ms`
    /// from a retention policy.
    pub fn prune_history(&self, cutoff_ms: i64) -> SqliteResult<usize> {
        self.conn.execute(
            "DELETE FROM history WHERE created_at_ms < ?1",
            params![cutoff_ms],
        )
    }
}

/// Escape LIKE metacharacters (`%`, `_`, and the escape character itself,
/// `\`) in `input` so it can be safely embedded in a `LIKE ... ESCAPE '\'`
/// pattern and matched literally rather than as a wildcard.
fn escape_like(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for c in input.chars() {
        if matches!(c, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Pure computation of the retention cutoff timestamp: rows with
/// `created_at_ms` older than the returned value are eligible for
/// [`Store::prune_history`]. `now_ms` is injected (rather than read from
/// the system clock) so this stays unit-testable with a fixed instant.
///
/// `retention_days == 0` means "keep forever" — `None`, not a cutoff of
/// `now_ms`, so callers must treat `None` as "don't prune" rather than
/// accidentally pruning everything.
pub fn retention_cutoff_ms(now_ms: i64, retention_days: u32) -> Option<i64> {
    if retention_days == 0 {
        return None;
    }
    let retention_ms = i64::from(retention_days) * 24 * 60 * 60 * 1000;
    Some(now_ms - retention_ms)
}

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
        store
            .insert_history(1_000, "alpha", "alpha.", None)
            .unwrap();

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
        let keep = store
            .insert_history(1_000, "keep me", "keep me.", None)
            .unwrap();
        let drop = store
            .insert_history(2_000, "drop me", "drop me.", None)
            .unwrap();

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
