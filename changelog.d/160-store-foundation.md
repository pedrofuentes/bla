- Headless SQLite `Store` foundation for dictation history (kickoff #160): a
  numbered, idempotent `PRAGMA user_version` migration runner creates the
  `history` table (raw + cleaned text, timestamp, source app), with
  insert/search/delete/clear operations and a pure `retention_cutoff_ms`
  helper for future auto-pruning. Backend-only — no settings UI or commands
  yet; those land in a later M3 PR.
