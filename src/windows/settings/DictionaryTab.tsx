import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { invoke, type DictionaryTerm } from "../../lib/ipc";

type LoadState = "loading" | "ready" | "error";

/**
 * Every error string this component ever renders is one of these statics —
 * never a raw backend error message — mirroring the privacy rule that
 * dictionary terms (and, by extension, anything derived from a failed call
 * touching them) must never leak into a rendered string (MISSION §5/§7;
 * see PR #223's Sentinel digest, issue #227, for the gap this avoids).
 */
const LOAD_ERROR_MESSAGE = "Couldn't load your dictionary. Try again.";
const ADD_ERROR_MESSAGE = "Couldn't add that term. Try again.";
const REMOVE_ERROR_MESSAGE = "Couldn't remove that term. Try again.";
const EMPTY_INPUT_MESSAGE = "Enter a term to add.";
const DUPLICATE_MESSAGE = "That term is already in your dictionary.";

/**
 * Dictionary settings tab (issue #201, M3 PR 3.5): the user's personal
 * dictionary — vocabulary (names, product names, jargon, acronyms) fed to
 * Whisper's initial prompt and the cleanup rewrite pass, see
 * `store::DictionaryTerm`'s doc comment — rendered as a plain list with an
 * add-term input and per-term remove. Talks to the core only through
 * `src/lib/ipc.ts`, per docs/ARCHITECTURE.md §Module Boundaries.
 *
 * ## Add-term validation (AC-38/AC-39)
 *
 * `commands::add_dictionary_term` wraps `Store::add_term`, whose
 * `dictionary(term UNIQUE COLLATE NOCASE)` schema constraint makes a
 * case-insensitive duplicate an `INSERT OR IGNORE` no-op that still
 * *succeeds* (returns the existing row's id) — the backend never rejects a
 * duplicate add, so relying on a rejected call to detect one would never
 * fire in production. AC-39 ("surfaces inline validation feedback... rather
 * than silently failing or duplicating") is therefore enforced here,
 * client-side, against the already-loaded term list, before the backend is
 * ever called: a blank/whitespace-only submission or a case-insensitive
 * match against an existing term shows an inline error and the call to
 * `add_dictionary_term` never happens. A defensive `.catch` still handles
 * any genuinely failed call (e.g. the IPC round trip itself failing),
 * reverting to a clean retry state without discarding the user's draft.
 *
 * Privacy (MISSION §5/§7): terms are the user's own content — this
 * component never `console.log`s a term, and every rendered error is one
 * of the static, kind-derived strings above, never a raw backend message.
 */
export function DictionaryTab() {
  const [terms, setTerms] = useState<DictionaryTerm[] | null>(null);
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [loadError, setLoadError] = useState<string | null>(null);

  const [draftTerm, setDraftTerm] = useState("");
  const [adding, setAdding] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);

  const [removingId, setRemovingId] = useState<number | null>(null);
  const [removeError, setRemoveError] = useState<string | null>(null);

  const cancelledRef = useRef(false);

  useEffect(() => {
    cancelledRef.current = false;
    invoke("list_dictionary_terms")
      .then((rows) => {
        if (cancelledRef.current) return;
        setTerms(rows);
        setLoadState("ready");
        setLoadError(null);
      })
      .catch(() => {
        if (cancelledRef.current) return;
        setLoadState("error");
        setLoadError(LOAD_ERROR_MESSAGE);
      });

    return () => {
      cancelledRef.current = true;
    };
  }, []);

  const handleDraftChange = useCallback((value: string) => {
    setDraftTerm(value);
    // Clear a stale validation message once the user starts editing again
    // rather than leaving it stuck after they've already addressed it.
    setAddError((prev) => (prev ? null : prev));
  }, []);

  const handleAddSubmit = useCallback(
    (e: FormEvent) => {
      e.preventDefault();
      const trimmed = draftTerm.trim();

      if (trimmed === "") {
        setAddError(EMPTY_INPUT_MESSAGE);
        return;
      }
      const isDuplicate = (terms ?? []).some((t) => t.term.toLowerCase() === trimmed.toLowerCase());
      if (isDuplicate) {
        setAddError(DUPLICATE_MESSAGE);
        return;
      }

      setAddError(null);
      setAdding(true);
      invoke("add_dictionary_term", { term: trimmed })
        .then((id) => {
          if (cancelledRef.current) return;
          setTerms((prev) => [{ id, term: trimmed, created_at_ms: Date.now() }, ...(prev ?? [])]);
          setDraftTerm("");
        })
        .catch(() => {
          if (cancelledRef.current) return;
          setAddError(ADD_ERROR_MESSAGE);
        })
        .finally(() => {
          if (!cancelledRef.current) setAdding(false);
        });
    },
    [draftTerm, terms],
  );

  const handleRemove = useCallback((id: number) => {
    setRemoveError(null);
    setRemovingId(id);
    invoke("remove_dictionary_term", { id })
      .then(() => {
        if (cancelledRef.current) return;
        setTerms((prev) => (prev ? prev.filter((t) => t.id !== id) : prev));
      })
      .catch(() => {
        if (cancelledRef.current) return;
        setRemoveError(REMOVE_ERROR_MESSAGE);
      })
      .finally(() => {
        if (!cancelledRef.current) setRemovingId(null);
      });
  }, []);

  const isLoading = loadState === "loading" && terms === null;
  const isEmpty = loadState === "ready" && terms !== null && terms.length === 0;

  return (
    <div className="flex max-w-lg flex-col gap-6" data-testid="dictionary-panel">
      <div className="flex flex-col gap-1">
        <label htmlFor="dictionary-add-input" className="text-sm font-medium">
          Add a term
        </label>
        <form className="flex items-center gap-2" onSubmit={handleAddSubmit}>
          <input
            id="dictionary-add-input"
            data-testid="dictionary-add-input"
            type="text"
            value={draftTerm}
            placeholder="e.g. a name, product, or acronym…"
            onChange={(e) => handleDraftChange(e.target.value)}
            className="flex-1 rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:outline-none dark:border-neutral-700 dark:bg-neutral-950"
          />
          <button
            type="submit"
            data-testid="dictionary-add-button"
            disabled={adding}
            className="shrink-0 rounded-md bg-blue-600 px-3 py-2 text-sm font-medium text-white disabled:cursor-not-allowed disabled:opacity-50 hover:bg-blue-500"
          >
            {adding ? "Adding…" : "Add"}
          </button>
        </form>
        <p className="text-xs text-neutral-500 dark:text-neutral-400">
          Names, product names, jargon, and acronyms bla should recognize and spell the way you
          write them.
        </p>
        {addError && (
          <p data-testid="dictionary-add-error" className="text-xs text-red-600 dark:text-red-400">
            {addError}
          </p>
        )}
      </div>

      <div className="flex flex-col gap-2 border-t border-neutral-200 pt-4 dark:border-neutral-800">
        {isLoading && (
          <p
            data-testid="dictionary-loading"
            className="text-sm text-neutral-500 dark:text-neutral-400"
          >
            Loading…
          </p>
        )}

        {loadState === "error" && (
          <p data-testid="dictionary-load-error" className="text-xs text-red-600 dark:text-red-400">
            {loadError}
          </p>
        )}

        {isEmpty && (
          <p
            data-testid="dictionary-empty-state"
            className="text-sm text-neutral-500 dark:text-neutral-400"
          >
            No terms yet. Add one above.
          </p>
        )}

        {terms !== null && terms.length > 0 && (
          <ul className="flex flex-col gap-1" data-testid="dictionary-list">
            {terms.map((t) => (
              <li
                key={t.id}
                data-testid={`dictionary-term-${t.id}`}
                className="flex items-center justify-between gap-3 rounded-md border border-neutral-200 px-3 py-2 dark:border-neutral-800"
              >
                <span
                  data-testid={`dictionary-term-label-${t.id}`}
                  className="min-w-0 truncate text-sm text-neutral-900 dark:text-neutral-100"
                >
                  {t.term}
                </span>
                <button
                  type="button"
                  data-testid={`dictionary-remove-${t.id}`}
                  disabled={removingId === t.id}
                  onClick={() => handleRemove(t.id)}
                  className="shrink-0 rounded-md border border-neutral-300 px-2 py-1 text-xs font-medium text-red-600 disabled:cursor-not-allowed disabled:opacity-50 hover:bg-red-50 dark:border-neutral-700 dark:text-red-400 dark:hover:bg-red-950/30"
                >
                  {removingId === t.id ? "Removing…" : "Remove"}
                </button>
              </li>
            ))}
          </ul>
        )}

        {removeError && (
          <p
            data-testid="dictionary-remove-error"
            className="text-xs text-red-600 dark:text-red-400"
          >
            {removeError}
          </p>
        )}
      </div>
    </div>
  );
}
