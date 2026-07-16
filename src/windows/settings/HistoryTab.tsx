import { useCallback, useEffect, useRef, useState } from "react";
import { invoke, type HistoryRow, type Settings } from "../../lib/ipc";

/** Row cap passed to `search_history` — generous for a local single-user DB. */
const SEARCH_LIMIT = 200;

type LoadState = "loading" | "ready" | "error";
type RetentionSaveStatus = "idle" | "saving" | "saved";

/** `created_at_ms` → a compact, locale-independent "YYYY-MM-DD HH:mm" string. */
function formatTimestamp(createdAtMs: number): string {
  const iso = new Date(createdAtMs).toISOString();
  return `${iso.slice(0, 10)} ${iso.slice(11, 16)}`;
}

/** Parses a retention-days input string into a non-negative integer, defaulting to 0. */
function parseRetentionDays(raw: string): number {
  const parsed = Number.parseInt(raw, 10);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : 0;
}

/**
 * History settings tab (issue #199, M3 PR 3.3): a substring search over
 * dictation history (`search_history`), per-entry Copy/Delete
 * (`copy_history_entry`/`delete_history_entry`), a "Clear all" action
 * gated behind an inline confirm (never a native `window.confirm`), and a
 * retention-days control bound to `Settings.retention_days` (`0` = keep
 * forever). Talks to the core only through `src/lib/ipc.ts`, per
 * docs/ARCHITECTURE.md §Module Boundaries.
 *
 * ## Search (AC-32)
 *
 * Re-queries `search_history` on every keystroke (on-change, not
 * debounced — AC-32 allows either; the extra IPC round trips are cheap
 * against a local single-user SQLite DB). A monotonic `searchSeqRef` guard
 * discards a stale response that resolves after a newer query was already
 * issued, so fast typing can't flash an out-of-date result list.
 *
 * ## Copy/Delete/Clear-all (AC-33)
 *
 * Copy and Delete each call their command with just the entry's `id`; a
 * successful Delete splices the row out of local state directly — no
 * re-fetch — so the list updates without waiting on a round trip. "Clear
 * all" shows an inline confirm/cancel row instead of a native dialog
 * (design rubric: quiet, minimal, native-*feeling*, not an OS alert).
 *
 * ## Retention (AC-34)
 *
 * Mirrors `GeneralTab`'s file-mode text fields: a local draft committed on
 * blur (not per-keystroke) against the full `Settings` object loaded via
 * `get_settings`, so `set_settings` never clobbers fields this tab doesn't
 * own. A failed write reverts the draft to the last known-persisted value.
 *
 * Privacy (MISSION §5/§7): rows carry the user's own transcript text —
 * this component never `console.log`s a row, the search query, or a
 * search result; only static, kind-derived strings ever reach an error
 * state.
 */
export function HistoryTab() {
  const [rows, setRows] = useState<HistoryRow[] | null>(null);
  const [query, setQuery] = useState("");
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [copiedId, setCopiedId] = useState<number | null>(null);
  const [confirmingClear, setConfirmingClear] = useState(false);
  const [clearing, setClearing] = useState(false);

  const [retentionDraft, setRetentionDraft] = useState("0");
  const [retentionSaveStatus, setRetentionSaveStatus] = useState<RetentionSaveStatus>("idle");
  const [retentionError, setRetentionError] = useState<string | null>(null);

  const settingsRef = useRef<Settings | null>(null);
  const retentionInitializedRef = useRef(false);
  const cancelledRef = useRef(false);
  // Guards a stale search response (fast typing) from clobbering a newer one.
  const searchSeqRef = useRef(0);

  // Deliberately does not set `loadState` to `"loading"` synchronously here
  // (react-hooks/set-state-in-effect: this runs directly from the mount
  // effect below for the initial search) — the `useState("loading")`
  // initializer already covers the first call's loading state, and a
  // later re-query (from `handleQueryChange`, an event handler) only
  // flips `loadState` once its response actually resolves, matching the
  // async-continuation pattern every other IPC call in this codebase uses
  // (e.g. `GeneralTab`'s mount effect).
  const runSearch = useCallback((q: string) => {
    const seq = ++searchSeqRef.current;
    invoke("search_history", { query: q, limit: SEARCH_LIMIT })
      .then((results) => {
        if (cancelledRef.current || searchSeqRef.current !== seq) return;
        setRows(results);
        setLoadState("ready");
        setLoadError(null);
      })
      .catch((err) => {
        if (cancelledRef.current || searchSeqRef.current !== seq) return;
        setLoadState("error");
        setLoadError(String(err));
      });
  }, []);

  useEffect(() => {
    cancelledRef.current = false;
    runSearch("");

    invoke("get_settings")
      .then((loaded) => {
        if (cancelledRef.current) return;
        settingsRef.current = loaded;
        if (!retentionInitializedRef.current) {
          retentionInitializedRef.current = true;
          setRetentionDraft(String(loaded.retention_days ?? 0));
        }
      })
      .catch((err) => {
        if (!cancelledRef.current) setRetentionError(String(err));
      });

    return () => {
      cancelledRef.current = true;
    };
  }, [runSearch]);

  const handleQueryChange = useCallback(
    (value: string) => {
      setQuery(value);
      runSearch(value);
    },
    [runSearch],
  );

  const handleCopy = useCallback((id: number) => {
    setActionError(null);
    invoke("copy_history_entry", { id })
      .then(() => {
        if (!cancelledRef.current) setCopiedId(id);
      })
      .catch((err) => {
        if (!cancelledRef.current) setActionError(String(err));
      });
  }, []);

  const handleDelete = useCallback((id: number) => {
    setActionError(null);
    invoke("delete_history_entry", { id })
      .then(() => {
        if (cancelledRef.current) return;
        setRows((prev) => (prev ? prev.filter((row) => row.id !== id) : prev));
        setCopiedId((prev) => (prev === id ? null : prev));
      })
      .catch((err) => {
        if (!cancelledRef.current) setActionError(String(err));
      });
  }, []);

  const handleClearAll = useCallback(() => {
    setClearing(true);
    setActionError(null);
    invoke("clear_history")
      .then(() => {
        if (cancelledRef.current) return;
        setRows([]);
        setCopiedId(null);
        setConfirmingClear(false);
      })
      .catch((err) => {
        if (!cancelledRef.current) setActionError(String(err));
      })
      .finally(() => {
        if (!cancelledRef.current) setClearing(false);
      });
  }, []);

  const commitRetention = useCallback(() => {
    const base = settingsRef.current;
    const clamped = parseRetentionDays(retentionDraft);
    setRetentionDraft(String(clamped));
    if (!base) return;
    if ((base.retention_days ?? 0) === clamped) return;

    const next: Settings = { ...base, retention_days: clamped };
    settingsRef.current = next;
    setRetentionError(null);
    setRetentionSaveStatus("saving");
    invoke("set_settings", { settings: next })
      .then(() => {
        if (!cancelledRef.current) setRetentionSaveStatus("saved");
      })
      .catch((err) => {
        if (cancelledRef.current) return;
        setRetentionSaveStatus("idle");
        setRetentionError(String(err));
        // Revert to the last known-persisted value.
        settingsRef.current = base;
        setRetentionDraft(String(base.retention_days ?? 0));
      });
  }, [retentionDraft]);

  const isLoading = loadState === "loading" && rows === null;
  const isEmpty = loadState === "ready" && rows !== null && rows.length === 0;

  return (
    <div className="flex max-w-lg flex-col gap-6" data-testid="history-panel">
      <div className="flex flex-col gap-1">
        <label htmlFor="history-search-input" className="text-sm font-medium">
          Search
        </label>
        <input
          id="history-search-input"
          data-testid="history-search-input"
          type="text"
          value={query}
          placeholder="Search your dictation history…"
          onChange={(e) => handleQueryChange(e.target.value)}
          className="rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:outline-none dark:border-neutral-700 dark:bg-neutral-950"
        />
      </div>

      <div className="flex flex-col gap-2">
        {isLoading && (
          <p
            data-testid="history-loading"
            className="text-sm text-neutral-500 dark:text-neutral-400"
          >
            Loading…
          </p>
        )}

        {loadState === "error" && (
          <p data-testid="history-load-error" className="text-xs text-red-600 dark:text-red-400">
            {loadError}
          </p>
        )}

        {isEmpty && (
          <p
            data-testid="history-empty-state"
            className="text-sm text-neutral-500 dark:text-neutral-400"
          >
            {query.trim() === "" ? "No dictation history yet." : "No matching entries."}
          </p>
        )}

        {rows !== null && rows.length > 0 && (
          <ul className="flex flex-col gap-1" data-testid="history-list">
            {rows.map((row) => (
              <li
                key={row.id}
                data-testid={`history-row-${row.id}`}
                className="flex items-center justify-between gap-3 rounded-md border border-neutral-200 px-3 py-2 dark:border-neutral-800"
              >
                <div className="min-w-0 flex-1">
                  <p
                    data-testid={`history-meta-${row.id}`}
                    className="truncate text-xs text-neutral-500 dark:text-neutral-400"
                  >
                    {formatTimestamp(row.created_at_ms)}
                    {row.app_name ? ` · ${row.app_name}` : ""}
                  </p>
                  <p
                    data-testid={`history-preview-${row.id}`}
                    className="truncate text-sm text-neutral-900 dark:text-neutral-100"
                  >
                    {row.cleaned}
                  </p>
                </div>
                <div className="flex shrink-0 gap-1.5">
                  <button
                    type="button"
                    data-testid={`history-copy-${row.id}`}
                    onClick={() => handleCopy(row.id)}
                    className="rounded-md border border-neutral-300 px-2 py-1 text-xs font-medium hover:bg-neutral-100 dark:border-neutral-700 dark:hover:bg-neutral-800"
                  >
                    {copiedId === row.id ? "Copied" : "Copy"}
                  </button>
                  <button
                    type="button"
                    data-testid={`history-delete-${row.id}`}
                    onClick={() => handleDelete(row.id)}
                    className="rounded-md border border-neutral-300 px-2 py-1 text-xs font-medium text-red-600 hover:bg-red-50 dark:border-neutral-700 dark:text-red-400 dark:hover:bg-red-950/30"
                  >
                    Delete
                  </button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="flex flex-col gap-2 border-t border-neutral-200 pt-4 dark:border-neutral-800">
        {!confirmingClear ? (
          <button
            type="button"
            data-testid="history-clear-all-button"
            disabled={!rows || rows.length === 0}
            onClick={() => setConfirmingClear(true)}
            className="self-start rounded-md border border-neutral-300 px-3 py-1.5 text-xs font-medium text-red-600 disabled:cursor-not-allowed disabled:opacity-50 hover:bg-red-50 dark:border-neutral-700 dark:text-red-400 dark:hover:bg-red-950/30"
          >
            Clear all history
          </button>
        ) : (
          <div
            data-testid="history-clear-confirm"
            className="flex items-center gap-2 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-xs dark:border-red-900 dark:bg-red-950/30"
          >
            <span className="text-red-700 dark:text-red-300">
              Delete all history? This can&apos;t be undone.
            </span>
            <button
              type="button"
              data-testid="history-clear-confirm-button"
              disabled={clearing}
              onClick={handleClearAll}
              className="rounded-md bg-red-600 px-2 py-1 font-medium text-white disabled:cursor-not-allowed disabled:opacity-50 hover:bg-red-500"
            >
              {clearing ? "Clearing…" : "Confirm"}
            </button>
            <button
              type="button"
              data-testid="history-clear-cancel-button"
              disabled={clearing}
              onClick={() => setConfirmingClear(false)}
              className="rounded-md border border-neutral-300 px-2 py-1 font-medium disabled:cursor-not-allowed disabled:opacity-50 hover:bg-neutral-100 dark:border-neutral-700 dark:hover:bg-neutral-800"
            >
              Cancel
            </button>
          </div>
        )}

        {actionError && (
          <p data-testid="history-action-error" className="text-xs text-red-600 dark:text-red-400">
            {actionError}
          </p>
        )}
      </div>

      <div className="flex flex-col gap-1 border-t border-neutral-200 pt-4 dark:border-neutral-800">
        <label htmlFor="history-retention-input" className="text-sm font-medium">
          Keep history for
        </label>
        <div className="flex items-center gap-2">
          <input
            id="history-retention-input"
            data-testid="history-retention-input"
            type="number"
            min={0}
            step={1}
            inputMode="numeric"
            value={retentionDraft}
            onChange={(e) => setRetentionDraft(e.target.value)}
            onBlur={commitRetention}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
            }}
            className="w-24 rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:outline-none dark:border-neutral-700 dark:bg-neutral-950"
          />
          <span className="text-sm text-neutral-500 dark:text-neutral-400">days</span>
        </div>
        <p
          data-testid="history-retention-help"
          className="text-xs text-neutral-500 dark:text-neutral-400"
        >
          History older than this is deleted automatically. Use 0 to keep forever.
        </p>
        {retentionError && (
          <p
            data-testid="history-retention-error"
            className="text-xs text-red-600 dark:text-red-400"
          >
            {retentionError}
          </p>
        )}
        {retentionSaveStatus === "saved" && (
          <span
            data-testid="history-retention-saved"
            className="text-xs text-neutral-500 dark:text-neutral-400"
          >
            Saved ✓
          </span>
        )}
      </div>
    </div>
  );
}
