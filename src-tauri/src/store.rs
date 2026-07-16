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
//! It also derives `Serialize` (issue #198) so `commands::search_history`
//! can hand rows to the frontend over Tauri IPC — the one sanctioned
//! "leaves this module" path for history text (the History tab the user
//! opens to browse their own dictations, #199), distinct from logging: IPC
//! payloads never pass through `eprintln!`/`log!`/an emitted error event.

use rusqlite::{params, Connection, OptionalExtension, Result as SqliteResult};
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

/// One forward-only migration: the `user_version` it brings the schema to,
/// and the SQL that gets it there. Idempotent by construction — the runner
/// only ever applies a migration whose version is greater than the DB's
/// current `user_version`, so re-opening an already-migrated DB is a no-op.
///
/// Later M3 PRs (dictionary, tone_rules, snippets) add migrations 2/3/4 by
/// appending entries here — nothing about the runner itself needs to change.
type Migration = (i64, &'static str);

const MIGRATIONS: &[Migration] = &[
    (
        1,
        "CREATE TABLE IF NOT EXISTS history (
            id INTEGER PRIMARY KEY,
            created_at_ms INTEGER NOT NULL,
            raw TEXT NOT NULL,
            cleaned TEXT NOT NULL,
            app_name TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_history_created_at_ms ON history(created_at_ms);",
    ),
    (
        2,
        // Issue #200 (PRD AC-21), schema per #160's plan note:
        // `dictionary(term UNIQUE NOCASE)`. `term`'s column-level `UNIQUE
        // COLLATE NOCASE` constraint is what makes `add_term`'s
        // case-insensitive de-duplication ("Kubernetes" then "kubernetes"
        // is a no-op, not two rows) a property of the schema itself rather
        // than something every caller has to remember to check for.
        "CREATE TABLE IF NOT EXISTS dictionary (
            id INTEGER PRIMARY KEY,
            term TEXT NOT NULL UNIQUE COLLATE NOCASE,
            created_at_ms INTEGER NOT NULL
        );",
    ),
];

/// A single dictation history entry (issue #160).
///
/// Derives `Debug` for test assertions only — no code path in this crate
/// logs/prints a `HistoryRow` (MISSION §5/§7: raw/cleaned dictation text
/// must never be logged, even though storing it in local SQLite is
/// sanctioned).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HistoryRow {
    pub id: i64,
    pub created_at_ms: i64,
    pub raw: String,
    pub cleaned: String,
    pub app_name: Option<String>,
}

/// One term in the user's personal dictionary (issue #200, PRD AC-21):
/// vocabulary — names, product names, jargon, acronyms — fed to Whisper's
/// `initial_prompt` ([`crate::stt::build_initial_prompt`]) and to the
/// `cleanup_v2` rewrite prompt ([`crate::cleanup::render_cleanup_prompt_v2`])
/// to bias recognition and spelling correction toward the user's own words.
///
/// Derives `Debug` for test assertions only, mirroring [`HistoryRow`]'s own
/// doc comment: dictionary terms are user content and stay under the same
/// no-log invariant (MISSION §5/§7) as transcript text — nothing in this
/// crate `println!`/`log!`s a `DictionaryTerm`. Also derives `Serialize`
/// (like `HistoryRow`) so `commands::list_dictionary_terms` can hand rows to
/// the frontend over Tauri IPC — the one sanctioned "leaves this module"
/// path, the user's own Dictionary tab (#201).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DictionaryTerm {
    pub id: i64,
    pub term: String,
    pub created_at_ms: i64,
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
        // Issue #162 (SNTL-20260713-bla-PR161-b26d368): set explicitly,
        // before `migrate()`, so a transient lock on the real on-disk DB
        // this connection is about to be wired to on the dictation hot path
        // (second app instance — no single-instance guard exists; a
        // crash-relaunch race; an OS indexer/backup read lock) makes the
        // next write BLOCK AND RETRY for up to 5s instead of failing
        // immediately with `SQLITE_BUSY` and dropping a history row.
        conn.busy_timeout(Duration::from_secs(5))?;
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

    /// The connection's current `PRAGMA busy_timeout`, in milliseconds — the
    /// value issue #162's test asserts against to confirm
    /// [`Self::from_connection`] sets it before any caller can observe a
    /// default of `0`.
    #[cfg(test)]
    fn busy_timeout_ms(&self) -> SqliteResult<i64> {
        self.conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
    }

    /// Apply every migration in [`MIGRATIONS`] whose version is greater
    /// than the DB's current `user_version`, in order, each inside its own
    /// transaction. Forward-only and idempotent: re-running against an
    /// already-migrated DB applies nothing and errors on nothing.
    ///
    /// Issue #163 (SNTL-20260713-bla-PR161-b26d368): also maintains a
    /// `schema_migrations` ledger recording exactly which versions have
    /// been applied. Migration 1's SQL happens to be idempotent on its own
    /// (`CREATE TABLE/INDEX IF NOT EXISTS`), so the `if *version <= current
    /// { continue; }` guard below had no test that could actually tell it
    /// apart from not existing at all. `INSERT INTO schema_migrations` is
    /// deliberately NOT idempotent (`version` is a `PRIMARY KEY`): if that
    /// guard is ever silently broken, a migration gets re-applied, its
    /// ledger insert hits a PRIMARY KEY violation, and `Store::open`
    /// returns `Err` instead of quietly re-running SQL with no visible
    /// effect — a failure a test (or a real reopen) can observe directly.
    fn migrate(&mut self) -> SqliteResult<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY NOT NULL
            );",
        )?;

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
            tx.execute(
                "INSERT INTO schema_migrations (version) VALUES (?1)",
                params![version],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    /// The set of migration versions [`Self::migrate`] has actually applied
    /// through the `schema_migrations` ledger, oldest first — the
    /// discriminating assertion issue #163 asks for (see `migrate`'s doc
    /// comment).
    #[cfg(test)]
    fn applied_migration_versions(&self) -> SqliteResult<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT version FROM schema_migrations ORDER BY version")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
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

    /// Fetch a single history row by id, or `None` if no row has that id.
    /// Issue #198 (AC-30): `copy_history_entry` uses this to read a row's
    /// `cleaned` text before routing it through the clipboard.
    pub fn get_history(&self, id: i64) -> SqliteResult<Option<HistoryRow>> {
        self.conn
            .query_row(
                "SELECT id, created_at_ms, raw, cleaned, app_name FROM history WHERE id = ?1",
                params![id],
                |row| {
                    Ok(HistoryRow {
                        id: row.get(0)?,
                        created_at_ms: row.get(1)?,
                        raw: row.get(2)?,
                        cleaned: row.get(3)?,
                        app_name: row.get(4)?,
                    })
                },
            )
            .optional()
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

    /// The most recent `history` row's `created_at_ms`, or `None` if
    /// `history` is empty. [`retention_cutoff_ms`]'s clock-skew guard
    /// (issue #219) uses this to sanity-check the computed cutoff against
    /// data that has actually been recorded.
    pub fn newest_history_timestamp(&self) -> SqliteResult<Option<i64>> {
        self.conn
            .query_row("SELECT MAX(created_at_ms) FROM history", [], |row| {
                row.get(0)
            })
    }

    /// Add `term` to the personal dictionary (issue #200, PRD AC-21).
    /// Case-insensitively unique (schema: `term UNIQUE COLLATE NOCASE`) —
    /// adding a term that already exists under a different case
    /// ("Kubernetes" then "kubernetes") is a no-op, not a second row or an
    /// error: the first-inserted casing and `created_at_ms` win. Returns
    /// the row id either way, so a caller always gets a stable id back for
    /// the term it asked for, whether that add just happened or a
    /// case-insensitive match already existed.
    pub fn add_term(&self, term: &str, created_at_ms: i64) -> SqliteResult<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO dictionary (term, created_at_ms) VALUES (?1, ?2)",
            params![term, created_at_ms],
        )?;
        // The column's own `COLLATE NOCASE` makes this lookup
        // case-insensitive too, so this resolves to the existing row on a
        // conflict without needing an explicit `COLLATE` in the query.
        self.conn.query_row(
            "SELECT id FROM dictionary WHERE term = ?1",
            params![term],
            |row| row.get(0),
        )
    }

    /// All dictionary terms, most-recently-added first.
    ///
    /// Ordering is a deliberate policy choice (issue #70): `build_initial_prompt`
    /// packs terms into Whisper's `initial_prompt` in the order it's given
    /// and skips whichever ones don't fit the length cap. Feeding it terms
    /// newest-first means that when the whole dictionary doesn't fit, it's
    /// the OLDEST terms that get skipped — recently-added terms are more
    /// likely to be what the user is actively dictating about (new jargon,
    /// a name they just added), so they should win a place over older ones
    /// rather than losing out to an arbitrary/insertion-order truncation.
    pub fn list_terms(&self) -> SqliteResult<Vec<DictionaryTerm>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, term, created_at_ms FROM dictionary
             ORDER BY created_at_ms DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(DictionaryTerm {
                id: row.get(0)?,
                term: row.get(1)?,
                created_at_ms: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    /// Remove a single dictionary term by id. Removing an id that doesn't
    /// exist is a no-op, not an error (mirrors [`Self::delete_history`]).
    pub fn remove_term(&self, id: i64) -> SqliteResult<()> {
        self.conn
            .execute("DELETE FROM dictionary WHERE id = ?1", params![id])?;
        Ok(())
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
///
/// `newest_row_ms` — [`Store::newest_history_timestamp`]'s result — is a
/// clock-skew guard (issue #219, SNTL-20260715-bla-PR218-cc04f8b): a
/// backward clock jump (or bad system time) can otherwise compute a cutoff
/// in the apparent future relative to every row that actually exists, and
/// `prune_history` would then delete all of history. Passing `None` (no
/// rows to check against, or a caller that hasn't been updated) disables
/// the guard and reproduces the pre-#219 behavior exactly — pruning an
/// empty table deletes nothing regardless, so this is never unsafe. Given
/// `Some(newest)`, two clamp/skip rules apply, in order:
///
/// 1. If `now_ms` is itself before `newest` — proof the clock has skewed
///    backward relative to data that was already recorded (a row can only
///    ever be inserted at `now_ms` at the time) — pruning is skipped
///    entirely (`None`): `now` can no longer be trusted to compute *any*
///    safe cutoff.
/// 2. Otherwise, the raw cutoff is clamped so it never exceeds `newest` —
///    guaranteeing the single most-recently-recorded row can never be
///    pruned by a miscalculated cutoff, however large `now_ms` or
///    `retention_days` turn out to be.
pub fn retention_cutoff_ms(
    now_ms: i64,
    retention_days: u32,
    newest_row_ms: Option<i64>,
) -> Option<i64> {
    if retention_days == 0 {
        return None;
    }
    let retention_ms = i64::from(retention_days) * 24 * 60 * 60 * 1000;
    let raw_cutoff = now_ms - retention_ms;

    if let Some(newest) = newest_row_ms {
        if now_ms < newest {
            return None;
        }
        if raw_cutoff > newest {
            return Some(newest);
        }
    }

    Some(raw_cutoff)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------
    // Migrations
    // -------------------------------------------------------------

    #[test]
    fn opening_an_in_memory_store_runs_every_migration_and_sets_user_version_to_the_latest() {
        let store = Store::open_in_memory().expect("open_in_memory should succeed");
        assert_eq!(store.schema_version().unwrap(), 3);
    }

    // -------------------------------------------------------------
    // Issue #163 (SNTL-20260713-bla-PR161-b26d368): the migration-idempotence
    // test gap. Migration 1's SQL (`CREATE TABLE/INDEX IF NOT EXISTS`) is
    // naturally idempotent, so re-running it was never observable —
    // "reopening is idempotent" passed even with the
    // `if *version <= current { continue; }` guard deleted, giving that
    // guard zero discriminating coverage. `schema_migrations` is a ledger
    // of exactly which versions have been applied, with `version` as a
    // PRIMARY KEY: `INSERT INTO schema_migrations` is NOT idempotent, so a
    // broken guard now surfaces as a hard `Store::open` error (a PRIMARY KEY
    // violation) the very next time a DB is reopened, rather than silently
    // re-running migration SQL with no visible effect.
    // -------------------------------------------------------------

    #[test]
    fn fresh_db_migration_ledger_records_every_migration_exactly_once_issue_163() {
        let store = Store::open_in_memory().expect("open_in_memory should succeed");
        assert_eq!(store.applied_migration_versions().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn reopening_a_store_twice_does_not_reapply_migrations_issue_163() {
        // The discriminating case: if the version guard were ever silently
        // broken, this SECOND open would attempt to re-run migration 1 and
        // 2's SQL, including a second `INSERT INTO schema_migrations`,
        // which would violate that table's PRIMARY KEY on `version` and
        // turn this `.expect` into a panic — a failure visible right here,
        // not just in a hypothetical future production reopen.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");

        {
            Store::open(&path).expect("first open should succeed");
        }
        let store = Store::open(&path)
            .expect("second open must not reapply migrations (would violate schema_migrations' PRIMARY KEY if the guard were broken)");

        assert_eq!(store.schema_version().unwrap(), 3);
        assert_eq!(store.applied_migration_versions().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn upgrading_a_pre_existing_v1_db_applies_only_migration_2_and_3_issue_163() {
        // Simulates a real-world upgrade: a DB created by code that only
        // knew about migration 1 (no `schema_migrations` ledger existed
        // yet) is then opened by the current code. Migration 1 must NOT be
        // re-applied (and isn't retroactively recorded in the ledger —
        // only migrations that actually ran through this runner are); only
        // migrations 2 (dictionary) and 3 (tone_rules) should apply.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0].1).unwrap();
            conn.execute_batch("PRAGMA user_version = 1;").unwrap();
        }

        let store = Store::open(&path).expect("upgrade open should succeed");
        assert_eq!(store.schema_version().unwrap(), 3);
        assert_eq!(store.applied_migration_versions().unwrap(), vec![2, 3]);

        // Migration 2's table is now actually usable.
        let id = store.add_term("Kubernetes", 1_000).unwrap();
        assert!(id > 0);
        // Migration 3's table is now actually usable too.
        let rule_id = store
            .upsert_tone_rule("Notes", ToneProfile::Casual, 1_000)
            .unwrap();
        assert!(rule_id > 0);
    }

    // -------------------------------------------------------------
    // Issue #202 (PRD AC-22, M3 per-app tone): migration 3 — `tone_rules`.
    // Mirrors #163's discriminating pattern for this new migration.
    // -------------------------------------------------------------

    #[test]
    fn upgrading_a_pre_existing_v2_db_applies_only_migration_3_issue_202() {
        // A DB already migrated to v2 by code that predates tone_rules
        // (issue #200's own state) must, on the next open, apply ONLY
        // migration 3 — not re-run 1/2.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");
        {
            let store = Store::open(&path).expect("v1+v2 open should succeed");
            assert_eq!(store.schema_version().unwrap(), 3);
        }
        // Roll the on-disk DB back to pretend it only ever reached v2: undo
        // migration 3's ledger row and table, then reset user_version.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "DROP TABLE tone_rules;
                 DELETE FROM schema_migrations WHERE version = 3;
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        }

        let store = Store::open(&path).expect("upgrade open should succeed");
        assert_eq!(store.schema_version().unwrap(), 3);
        assert_eq!(store.applied_migration_versions().unwrap(), vec![1, 2, 3]);

        let rule_id = store
            .upsert_tone_rule("Slack", ToneProfile::Casual, 1_000)
            .unwrap();
        assert!(rule_id > 0);
    }

    #[test]
    fn tone_rules_check_constraint_rejects_a_value_outside_the_three_profiles_ac41() {
        // AC-41: the CHECK constraint restricts `tone` to exactly
        // casual/formal/verbatim at the schema level — defense in depth
        // beneath the Rust `ToneProfile` enum (which can never construct an
        // invalid value in the first place), proven here via a raw INSERT
        // that bypasses the typed API entirely.
        let store = Store::open_in_memory().unwrap();
        let result = store.raw_execute_for_test(
            "INSERT INTO tone_rules (app_pattern, tone, created_at_ms) \
             VALUES ('Bogus App', 'sarcastic', 1000)",
        );
        assert!(
            result.is_err(),
            "a tone value outside the three profiles must be rejected by the CHECK constraint"
        );
    }

    #[test]
    fn upsert_then_list_tone_rules_round_trips_the_rule_ac41() {
        let store = Store::open_in_memory().unwrap();
        let id = store
            .upsert_tone_rule("Slack", ToneProfile::Casual, 1_000)
            .unwrap();
        assert!(id > 0);

        let rules = store.list_tone_rules().unwrap();
        assert_eq!(
            rules,
            vec![ToneRule {
                id,
                app_pattern: "Slack".to_string(),
                tone: ToneProfile::Casual,
                created_at_ms: 1_000,
            }]
        );
    }

    #[test]
    fn upsert_tone_rule_with_the_same_pattern_updates_the_tone_in_place_ac41() {
        // "upsert": re-submitting the same app_pattern with a different
        // tone must UPDATE the existing rule, not add a second row — an
        // edited rule takes effect on the very next dictation with no
        // restart required (AC-41).
        let store = Store::open_in_memory().unwrap();
        let first_id = store
            .upsert_tone_rule("Slack", ToneProfile::Casual, 1_000)
            .unwrap();
        let second_id = store
            .upsert_tone_rule("Slack", ToneProfile::Formal, 2_000)
            .unwrap();

        assert_eq!(first_id, second_id, "both calls must resolve to the same row");

        let rules = store.list_tone_rules().unwrap();
        assert_eq!(rules.len(), 1, "an edit must not add a second row");
        assert_eq!(rules[0].tone, ToneProfile::Formal, "the new tone must win");
    }

    #[test]
    fn upsert_tone_rule_matches_the_pattern_case_insensitively_like_the_dictionary_does() {
        // Mirrors the dictionary's `UNIQUE COLLATE NOCASE` semantics
        // (issue #160's schema note, applied here to app_pattern): re-
        // upserting under a different case must resolve to the same row,
        // not create a duplicate.
        let store = Store::open_in_memory().unwrap();
        let first_id = store
            .upsert_tone_rule("Slack", ToneProfile::Casual, 1_000)
            .unwrap();
        let second_id = store
            .upsert_tone_rule("slack", ToneProfile::Formal, 2_000)
            .unwrap();

        assert_eq!(first_id, second_id);
        assert_eq!(store.list_tone_rules().unwrap().len(), 1);
    }

    #[test]
    fn distinct_app_patterns_are_not_treated_as_duplicates_ac41() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_tone_rule("Slack", ToneProfile::Casual, 1_000)
            .unwrap();
        store
            .upsert_tone_rule("Mail", ToneProfile::Formal, 2_000)
            .unwrap();

        assert_eq!(store.list_tone_rules().unwrap().len(), 2);
    }

    #[test]
    fn list_tone_rules_orders_by_insertion_order() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_tone_rule("Slack", ToneProfile::Casual, 1_000)
            .unwrap();
        store
            .upsert_tone_rule("Mail", ToneProfile::Formal, 2_000)
            .unwrap();
        store
            .upsert_tone_rule("Terminal", ToneProfile::Verbatim, 3_000)
            .unwrap();

        let patterns: Vec<String> = store
            .list_tone_rules()
            .unwrap()
            .into_iter()
            .map(|r| r.app_pattern)
            .collect();
        assert_eq!(patterns, vec!["Slack", "Mail", "Terminal"]);
    }

    #[test]
    fn delete_tone_rule_removes_only_the_targeted_row_ac41() {
        let store = Store::open_in_memory().unwrap();
        let keep = store
            .upsert_tone_rule("Slack", ToneProfile::Casual, 1_000)
            .unwrap();
        let drop = store
            .upsert_tone_rule("Mail", ToneProfile::Formal, 2_000)
            .unwrap();

        store.delete_tone_rule(drop).unwrap();

        let rules = store.list_tone_rules().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, keep);
    }

    #[test]
    fn delete_tone_rule_on_a_nonexistent_id_is_a_noop_ac41() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_tone_rule("Slack", ToneProfile::Casual, 1_000)
            .unwrap();

        store.delete_tone_rule(999_999).unwrap();

        assert_eq!(store.list_tone_rules().unwrap().len(), 1);
    }

    // -------------------------------------------------------------
    // Issue #162 (SNTL-20260713-bla-PR161-b26d368): `from_connection` never
    // called `Connection::busy_timeout` explicitly, so a transient lock
    // (second app instance, crash-relaunch race, OS indexer/backup read
    // lock) could make the next write fail immediately with `SQLITE_BUSY`
    // and drop a history row, instead of blocking up to `busy_timeout` for
    // the lock to clear.
    //
    // Note on this test's red/green shape: as vendored, rusqlite 0.40.1
    // already calls `sqlite3_busy_timeout(db, 5000)` unconditionally inside
    // `InnerConnection::open_with_flags` (see
    // `inner_connection.rs`) — so this assertion is green even before
    // `from_connection` makes the call explicit below. The explicit call is
    // still required per #162: it's a documented contract this crate owns
    // rather than an incidental upstream default an unrelated future
    // rusqlite bump could silently change, and it's what a reviewer
    // (Sentinel) can see and verify at this call site without reading
    // rusqlite's internals.
    // -------------------------------------------------------------

    #[test]
    fn opening_a_store_sets_a_five_second_busy_timeout_issue_162() {
        let store = Store::open_in_memory().expect("open_in_memory should succeed");
        assert_eq!(
            store.busy_timeout_ms().unwrap(),
            5_000,
            "busy_timeout must be set to 5s so a transient lock blocks and retries instead of \
             failing the write immediately with SQLITE_BUSY"
        );
    }

    #[test]
    fn reopening_an_on_disk_store_is_idempotent_and_keeps_user_version_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");

        {
            let store = Store::open(&path).expect("first open should succeed");
            assert_eq!(store.schema_version().unwrap(), 2);
        }

        // Re-opening an already-migrated DB must not error and must leave
        // user_version untouched — the migration runner is forward-only and
        // idempotent.
        let store = Store::open(&path).expect("second open should succeed");
        assert_eq!(store.schema_version().unwrap(), 2);
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
    // get_history (issue #198, AC-30): copy_history_entry needs to fetch a
    // single row by id to read its `cleaned` text before routing it through
    // the clipboard.
    // -------------------------------------------------------------

    #[test]
    fn get_history_returns_the_row_matching_the_given_id() {
        let store = Store::open_in_memory().unwrap();
        let id = store
            .insert_history(1_000, "hello world", "Hello, world.", Some("Notes"))
            .unwrap();

        let row = store.get_history(id).unwrap();
        assert_eq!(
            row,
            Some(HistoryRow {
                id,
                created_at_ms: 1_000,
                raw: "hello world".to_string(),
                cleaned: "Hello, world.".to_string(),
                app_name: Some("Notes".to_string()),
            })
        );
    }

    #[test]
    fn get_history_returns_none_for_an_id_that_does_not_exist() {
        let store = Store::open_in_memory().unwrap();
        store
            .insert_history(1_000, "alpha", "alpha.", None)
            .unwrap();

        assert_eq!(store.get_history(999).unwrap(), None);
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

    // -------------------------------------------------------------
    // Personal dictionary CRUD (issue #200, PRD AC-21, AC-37)
    // -------------------------------------------------------------

    #[test]
    fn add_term_then_list_terms_round_trips_the_term() {
        let store = Store::open_in_memory().unwrap();
        let id = store.add_term("Kubernetes", 1_000).unwrap();
        assert!(id > 0);

        let terms = store.list_terms().unwrap();
        assert_eq!(
            terms,
            vec![DictionaryTerm {
                id,
                term: "Kubernetes".to_string(),
                created_at_ms: 1_000,
            }]
        );
    }

    #[test]
    fn adding_a_term_twice_with_different_casing_is_a_case_insensitive_no_op_issue_160() {
        // #160's schema note: `dictionary(term UNIQUE NOCASE)`. Adding
        // "Kubernetes" then "kubernetes" must be a no-op/conflict, not two
        // rows — the first-inserted casing and timestamp win.
        let store = Store::open_in_memory().unwrap();
        let first_id = store.add_term("Kubernetes", 1_000).unwrap();
        let second_id = store.add_term("kubernetes", 2_000).unwrap();

        assert_eq!(
            first_id, second_id,
            "both calls must resolve to the same row"
        );

        let terms = store.list_terms().unwrap();
        assert_eq!(
            terms.len(),
            1,
            "a case-insensitive duplicate must not add a second row"
        );
        assert_eq!(
            terms[0].term, "Kubernetes",
            "the first-inserted casing must win"
        );
        assert_eq!(
            terms[0].created_at_ms, 1_000,
            "the first-inserted timestamp must win"
        );
    }

    #[test]
    fn distinct_terms_are_not_treated_as_duplicates() {
        let store = Store::open_in_memory().unwrap();
        store.add_term("Kubernetes", 1_000).unwrap();
        store.add_term("kubectl", 2_000).unwrap();

        assert_eq!(store.list_terms().unwrap().len(), 2);
    }

    #[test]
    fn list_terms_orders_most_recently_added_first() {
        // Issue #70's chosen tie-break policy (documented on
        // `Store::list_terms`): when `build_initial_prompt`'s length cap
        // means not every dictionary term fits Whisper's prompt budget,
        // the most-recently-added terms should be the ones tried first.
        let store = Store::open_in_memory().unwrap();
        store.add_term("oldest", 1_000).unwrap();
        store.add_term("middle", 2_000).unwrap();
        store.add_term("newest", 3_000).unwrap();

        let terms: Vec<String> = store
            .list_terms()
            .unwrap()
            .into_iter()
            .map(|t| t.term)
            .collect();
        assert_eq!(terms, vec!["newest", "middle", "oldest"]);
    }

    #[test]
    fn remove_term_removes_only_the_targeted_row() {
        let store = Store::open_in_memory().unwrap();
        let keep = store.add_term("keep", 1_000).unwrap();
        let drop = store.add_term("drop", 2_000).unwrap();

        store.remove_term(drop).unwrap();

        let terms = store.list_terms().unwrap();
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].id, keep);
    }

    #[test]
    fn remove_term_on_a_nonexistent_id_is_a_noop() {
        let store = Store::open_in_memory().unwrap();
        store.add_term("keep", 1_000).unwrap();

        store.remove_term(999_999).unwrap();

        assert_eq!(store.list_terms().unwrap().len(), 1);
    }

    #[test]
    fn retention_cutoff_ms_of_zero_days_means_keep_forever() {
        assert_eq!(retention_cutoff_ms(1_000_000, 0, None), None);
    }

    #[test]
    fn retention_cutoff_ms_of_positive_days_subtracts_the_injected_now() {
        let now_ms: i64 = 10 * 24 * 60 * 60 * 1000; // day 10
        let cutoff = retention_cutoff_ms(now_ms, 3, None).unwrap();
        assert_eq!(cutoff, 7 * 24 * 60 * 60 * 1000); // day 7
    }

    // -------------------------------------------------------------
    // Issue #219 (SNTL-20260715-bla-PR218-cc04f8b): clock-skew hardening.
    // Without a guard, a backward clock jump (or bad system time) can make
    // the retention cutoff computed from wall-clock `now_ms` land in the
    // apparent future relative to every row that actually exists, and
    // `prune_history` would then delete all of history. `retention_cutoff_ms`
    // takes the newest recorded row's timestamp as a sanity check and
    // applies clamp/skip semantics against it.
    // -------------------------------------------------------------

    #[test]
    fn retention_cutoff_ms_is_unaffected_by_the_guard_when_the_clock_is_sane() {
        // Normal case: `now` is safely after the newest row, and the
        // computed cutoff doesn't exceed it either — the guard must be a
        // complete no-op here, identical to passing `None`.
        let now_ms: i64 = 10 * 24 * 60 * 60 * 1000; // day 10
        let newest_row_ms = 9 * 24 * 60 * 60 * 1000; // day 9
        let cutoff = retention_cutoff_ms(now_ms, 3, Some(newest_row_ms)).unwrap();
        assert_eq!(cutoff, 7 * 24 * 60 * 60 * 1000); // day 7, same as the unguarded case
    }

    #[test]
    fn retention_cutoff_ms_skips_pruning_when_now_is_before_the_newest_recorded_row_issue_219() {
        // Direct proof of a backward clock skew: `now` can never
        // legitimately read earlier than a timestamp that was already
        // recorded (rows are inserted at `now` at the time). When it does,
        // "now" cannot be trusted to compute any cutoff at all, so pruning
        // must be skipped entirely (`None`) rather than compute a
        // plausible-looking but untrustworthy value.
        let now_ms = 1_000;
        let newest_row_ms = 5_000;
        assert_eq!(retention_cutoff_ms(now_ms, 1, Some(newest_row_ms)), None);
    }

    #[test]
    fn retention_cutoff_ms_clamps_to_the_newest_row_rather_than_exceeding_it_issue_219() {
        // A `now_ms` far enough in the future (bad system time, or a
        // corrected clock jump) can push the raw cutoff past every row
        // that exists, which would otherwise prune all of history. The
        // cutoff must never exceed the newest row's own timestamp, so that
        // row (the most recent one) can never be pruned by a
        // miscalculated cutoff.
        let now_ms: i64 = 100_000_000;
        let retention_days = 1; // 86_400_000 ms
        let newest_row_ms = 5_000;
        let raw_cutoff = now_ms - i64::from(retention_days) * 24 * 60 * 60 * 1000;
        assert!(
            raw_cutoff > newest_row_ms,
            "test setup must actually exercise the clamp"
        );

        assert_eq!(
            retention_cutoff_ms(now_ms, retention_days, Some(newest_row_ms)),
            Some(newest_row_ms)
        );
    }

    #[test]
    fn newest_history_timestamp_is_none_for_an_empty_store() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.newest_history_timestamp().unwrap(), None);
    }

    #[test]
    fn newest_history_timestamp_returns_the_max_created_at_ms() {
        let store = Store::open_in_memory().unwrap();
        store.insert_history(1_000, "a", "a.", None).unwrap();
        store.insert_history(3_000, "b", "b.", None).unwrap();
        store.insert_history(2_000, "c", "c.", None).unwrap();

        assert_eq!(store.newest_history_timestamp().unwrap(), Some(3_000));
    }

    #[test]
    fn clock_skew_backwards_jump_cannot_mass_delete_history_issue_219() {
        // End-to-end discriminating test at the Store level: rows recorded
        // with a correct clock, then a "now" that reads BEFORE those rows
        // (simulating the clock having jumped backward since) must not
        // delete anything, however large `retention_days` is.
        let store = Store::open_in_memory().unwrap();
        store.insert_history(10_000, "a", "a.", None).unwrap();
        store.insert_history(20_000, "b", "b.", None).unwrap();
        store.insert_history(30_000, "c", "c.", None).unwrap();

        let skewed_now_ms = 5_000; // before every row above
        let newest = store.newest_history_timestamp().unwrap();
        let cutoff = retention_cutoff_ms(skewed_now_ms, 365, newest);
        let deleted = match cutoff {
            Some(cutoff_ms) => store.prune_history(cutoff_ms).unwrap(),
            None => 0,
        };

        assert_eq!(deleted, 0, "a clock-skewed `now` must not prune anything");
        assert_eq!(store.search_history("", 10).unwrap().len(), 3);
    }
}
