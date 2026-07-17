import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { invoke, type Snippet } from "../../lib/ipc";

type LoadState = "loading" | "ready" | "error";

/**
 * Every error string this component ever renders is one of these statics —
 * never a raw backend error message — mirroring the privacy rule
 * `DictionaryTab.tsx`/`ToneTab.tsx` document: snippet triggers and bodies
 * are user content, so nothing derived from a failed call touching them may
 * leak into a rendered string (MISSION §5/§7).
 */
const LOAD_ERROR_MESSAGE = "Couldn't load your snippets. Try again.";
const ADD_ERROR_MESSAGE = "Couldn't add that snippet. Try again.";
const REMOVE_ERROR_MESSAGE = "Couldn't remove that snippet. Try again.";
const EMPTY_TRIGGER_MESSAGE = "Enter a trigger phrase to add.";
const EMPTY_BODY_MESSAGE = "Enter the text this trigger should expand to.";
const DUPLICATE_MESSAGE = "You already have a snippet for that trigger — edit it below instead.";
/**
 * Row-scoped edit-error messages (AC-54) — kept distinct from the add-form
 * messages above (and from each other) so a row's inline error is specific
 * to what actually went wrong: a blank trigger, a trigger colliding with a
 * DIFFERENT row (both withheld client-side, no network call — see
 * `commitEdit`'s doc comment), or a genuinely failed `update_snippet` call.
 */
const EDIT_EMPTY_TRIGGER_MESSAGE = "Enter a trigger phrase.";
const EDIT_DUPLICATE_MESSAGE = "You already have a snippet for that trigger.";
const EDIT_FAILED_MESSAGE = "Couldn't update that snippet. Try again.";

interface Draft {
  trigger: string;
  body: string;
}

function draftsFromRows(rows: Snippet[]): Record<number, Draft> {
  return Object.fromEntries(rows.map((r) => [r.id, { trigger: r.trigger, body: r.body }]));
}

/**
 * Snippets settings tab (issue #258/#261, M4, AC-51/AC-54): the user's
 * stored trigger -> body text expansions (see `store::Snippet`'s doc
 * comment; matching a spoken trigger against a transcript is
 * `snippets::match_snippet`'s job, issue #260, not this component) —
 * rendered as a list of inline-editable trigger/body pairs with an add form
 * and per-entry remove. Talks to the core only through `src/lib/ipc.ts`,
 * per docs/ARCHITECTURE.md §Module Boundaries.
 *
 * ## List order
 *
 * `list_snippets` returns rows most-recently-added first (mirrors
 * `DictionaryTab.tsx`'s newest-first list — UNLIKE `ToneTab.tsx`'s
 * insertion/match order, since a snippet's position carries no matching
 * semantics; #260's `match_snippet` documents its own list-order contract
 * independently). A newly added snippet is therefore prepended, matching
 * that same order.
 *
 * ## Add-snippet validation
 *
 * `commands::add_snippet` wraps `Store::add_snippet`, whose
 * `snippets(trigger UNIQUE COLLATE NOCASE)` schema constraint makes a
 * case-insensitive duplicate trigger an `INSERT OR IGNORE` no-op that still
 * *succeeds* (returns the existing row's id) — the backend never rejects a
 * duplicate add. So, mirroring `DictionaryTab.tsx`'s/`ToneTab.tsx`'s AC-39
 * pattern, a case-insensitive duplicate of an already-loaded trigger (and a
 * blank/whitespace-only trigger or body) is caught here, client-side,
 * before the backend is ever called.
 *
 * ## Inline editing (AC-54)
 *
 * Each row's trigger and body are plain editable fields, committed on
 * BLUR rather than per-keystroke (mirrors `GeneralTab.tsx`'s
 * `commitBaseDir`/`commitTemplate` pattern, issue #209/#210) — an in-flight
 * `update_snippet` call on every keystroke would be both wasteful and racy.
 * A blank trigger or a trigger colliding case-insensitively with a
 * DIFFERENT row is withheld client-side (a row-scoped inline error, no
 * network call) the same way the add form validates; unlike `add_snippet`,
 * `update_snippet` itself genuinely REJECTS a colliding trigger too (the
 * schema's `UNIQUE COLLATE NOCASE` constraint applies on UPDATE — see
 * `Store::update_snippet`'s doc comment), so the client-side check here is
 * a UX nicety, not the only guard. A no-op blur (unchanged trigger/body)
 * never calls the backend.
 *
 * A per-row generation counter (`editGenRef`) guards against two rapid,
 * out-of-order edits to the SAME row — the identical pattern
 * `ToneTab.tsx`'s `handleToneChange` uses: only the response matching the
 * row's latest request is applied, whether it resolves or rejects, so a
 * slow stale response can never clobber a faster newer one. A failed edit
 * reverts the row to its last-known-good trigger/body and shows a
 * row-scoped inline error; the row's fields and remove button are disabled
 * while its own edit is in flight.
 *
 * Privacy (MISSION §5/§7): triggers and bodies are the user's own content —
 * this component never `console.log`s either, and every rendered error is
 * one of the static, kind-derived strings above, never a raw backend
 * message.
 */
export function SnippetsTab() {
  const [snippets, setSnippets] = useState<Snippet[] | null>(null);
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [loadError, setLoadError] = useState<string | null>(null);

  const [drafts, setDrafts] = useState<Record<number, Draft>>({});

  const [draftTrigger, setDraftTrigger] = useState("");
  const [draftBody, setDraftBody] = useState("");
  const [adding, setAdding] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);

  const [removingId, setRemovingId] = useState<number | null>(null);
  const [removeError, setRemoveError] = useState<string | null>(null);

  const [savingId, setSavingId] = useState<number | null>(null);
  // Row id -> the specific static message for that row's current edit
  // error (see the EDIT_*_MESSAGE constants above); absent means no error.
  const [rowErrors, setRowErrors] = useState<Record<number, string>>({});

  const cancelledRef = useRef(false);
  // Per-row monotonic generation counter for the edit guard: bumped at the
  // START of each edit request to that row; a response is applied only if
  // it's still the LATEST generation minted for that row when it settles.
  const editGenRef = useRef<Map<number, number>>(new Map());

  useEffect(() => {
    cancelledRef.current = false;
    invoke("list_snippets")
      .then((rows) => {
        if (cancelledRef.current) return;
        setSnippets(rows);
        setDrafts(draftsFromRows(rows));
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

  const handleDraftTriggerChange = useCallback((value: string) => {
    setDraftTrigger(value);
    setAddError((prev) => (prev ? null : prev));
  }, []);

  const handleDraftBodyChange = useCallback((value: string) => {
    setDraftBody(value);
    setAddError((prev) => (prev ? null : prev));
  }, []);

  const handleAddSubmit = useCallback(
    (e: FormEvent) => {
      e.preventDefault();
      const trimmedTrigger = draftTrigger.trim();
      const trimmedBody = draftBody.trim();

      if (trimmedTrigger === "") {
        setAddError(EMPTY_TRIGGER_MESSAGE);
        return;
      }
      if (trimmedBody === "") {
        setAddError(EMPTY_BODY_MESSAGE);
        return;
      }
      const isDuplicate = (snippets ?? []).some(
        (s) => s.trigger.toLowerCase() === trimmedTrigger.toLowerCase(),
      );
      if (isDuplicate) {
        setAddError(DUPLICATE_MESSAGE);
        return;
      }

      setAddError(null);
      setAdding(true);
      invoke("add_snippet", { trigger: trimmedTrigger, body: trimmedBody })
        .then((id) => {
          if (cancelledRef.current) return;
          const newRow: Snippet = {
            id,
            trigger: trimmedTrigger,
            body: trimmedBody,
            created_at_ms: Date.now(),
          };
          setSnippets((prev) => [newRow, ...(prev ?? [])]);
          setDrafts((prev) => ({ ...prev, [id]: { trigger: trimmedTrigger, body: trimmedBody } }));
          setDraftTrigger("");
          setDraftBody("");
        })
        .catch(() => {
          if (cancelledRef.current) return;
          setAddError(ADD_ERROR_MESSAGE);
        })
        .finally(() => {
          if (!cancelledRef.current) setAdding(false);
        });
    },
    [draftTrigger, draftBody, snippets],
  );

  const handleRemove = useCallback((id: number) => {
    setRemoveError(null);
    setRemovingId(id);
    invoke("remove_snippet", { id })
      .then(() => {
        if (cancelledRef.current) return;
        setSnippets((prev) => (prev ? prev.filter((s) => s.id !== id) : prev));
        setDrafts((prev) => {
          const next = { ...prev };
          delete next[id];
          return next;
        });
      })
      .catch(() => {
        if (cancelledRef.current) return;
        setRemoveError(REMOVE_ERROR_MESSAGE);
      })
      .finally(() => {
        if (!cancelledRef.current) setRemovingId(null);
      });
  }, []);

  const setRowError = useCallback((id: number, message: string) => {
    setRowErrors((prev) => ({ ...prev, [id]: message }));
  }, []);

  const clearRowError = useCallback((id: number) => {
    setRowErrors((prev) => {
      if (!(id in prev)) return prev;
      const next = { ...prev };
      delete next[id];
      return next;
    });
  }, []);

  const handleEditFieldChange = useCallback(
    (id: number, field: keyof Draft, value: string) => {
      setDrafts((prev) => ({ ...prev, [id]: { ...prev[id], [field]: value } }));
      clearRowError(id);
    },
    [clearRowError],
  );

  const commitEdit = useCallback(
    (row: Snippet) => {
      const draft = drafts[row.id];
      if (!draft) return;
      const trimmedTrigger = draft.trigger.trim();
      const trimmedBody = draft.body.trim();

      if (trimmedTrigger === "") {
        setRowError(row.id, EDIT_EMPTY_TRIGGER_MESSAGE);
        return;
      }
      if (trimmedTrigger === row.trigger && trimmedBody === row.body) {
        // No-op: normalize the draft (trimmed) without a network call.
        setDrafts((prev) => ({
          ...prev,
          [row.id]: { trigger: trimmedTrigger, body: trimmedBody },
        }));
        return;
      }
      const isDuplicate = (snippets ?? []).some(
        (s) => s.id !== row.id && s.trigger.toLowerCase() === trimmedTrigger.toLowerCase(),
      );
      if (isDuplicate) {
        setRowError(row.id, EDIT_DUPLICATE_MESSAGE);
        return;
      }

      const previousTrigger = row.trigger;
      const previousBody = row.body;
      const generation = (editGenRef.current.get(row.id) ?? 0) + 1;
      editGenRef.current.set(row.id, generation);

      clearRowError(row.id);
      setSavingId(row.id);
      // Optimistic: reflect the change immediately (mirrors
      // `ToneTab.tsx`'s `handleToneChange`), reverted on failure or
      // superseded by a newer edit's own optimistic update.
      setSnippets((prev) =>
        prev
          ? prev.map((s) =>
              s.id === row.id ? { ...s, trigger: trimmedTrigger, body: trimmedBody } : s,
            )
          : prev,
      );
      setDrafts((prev) => ({ ...prev, [row.id]: { trigger: trimmedTrigger, body: trimmedBody } }));

      invoke("update_snippet", { id: row.id, trigger: trimmedTrigger, body: trimmedBody })
        .then(() => {
          if (cancelledRef.current) return;
          if (editGenRef.current.get(row.id) !== generation) return;
          setSavingId((prev) => (prev === row.id ? null : prev));
        })
        .catch(() => {
          if (cancelledRef.current) return;
          if (editGenRef.current.get(row.id) !== generation) return;
          setSnippets((prev) =>
            prev
              ? prev.map((s) =>
                  s.id === row.id ? { ...s, trigger: previousTrigger, body: previousBody } : s,
                )
              : prev,
          );
          setDrafts((prev) => ({
            ...prev,
            [row.id]: { trigger: previousTrigger, body: previousBody },
          }));
          setRowError(row.id, EDIT_FAILED_MESSAGE);
          setSavingId((prev) => (prev === row.id ? null : prev));
        });
    },
    [drafts, snippets, setRowError, clearRowError],
  );

  const isLoading = loadState === "loading" && snippets === null;
  const isEmpty = loadState === "ready" && snippets !== null && snippets.length === 0;

  return (
    <div className="flex max-w-lg flex-col gap-6" data-testid="snippets-panel">
      <div className="flex flex-col gap-1">
        <span className="text-sm font-medium">Add a snippet</span>
        <form className="flex flex-col gap-2" onSubmit={handleAddSubmit}>
          <input
            data-testid="snippets-add-trigger-input"
            type="text"
            value={draftTrigger}
            placeholder="Trigger, e.g. sig"
            onChange={(e) => handleDraftTriggerChange(e.target.value)}
            className="min-w-0 rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:outline-none dark:border-neutral-700 dark:bg-neutral-950"
          />
          <textarea
            data-testid="snippets-add-body-input"
            value={draftBody}
            placeholder="Text it expands to…"
            rows={2}
            onChange={(e) => handleDraftBodyChange(e.target.value)}
            className="min-w-0 resize-y rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:outline-none dark:border-neutral-700 dark:bg-neutral-950"
          />
          <button
            type="submit"
            data-testid="snippets-add-button"
            disabled={adding}
            className="shrink-0 self-start rounded-md bg-blue-600 px-3 py-2 text-sm font-medium text-white disabled:cursor-not-allowed disabled:opacity-50 hover:bg-blue-500"
          >
            {adding ? "Adding…" : "Add"}
          </button>
        </form>
        <p className="text-xs text-neutral-500 dark:text-neutral-400">
          A trigger phrase spoken during dictation, and the text bla should insert in its place.
        </p>
        {addError && (
          <p data-testid="snippets-add-error" className="text-xs text-red-600 dark:text-red-400">
            {addError}
          </p>
        )}
      </div>

      <div className="flex flex-col gap-2 border-t border-neutral-200 pt-4 dark:border-neutral-800">
        {isLoading && (
          <p
            data-testid="snippets-loading"
            className="text-sm text-neutral-500 dark:text-neutral-400"
          >
            Loading…
          </p>
        )}

        {loadState === "error" && (
          <p data-testid="snippets-load-error" className="text-xs text-red-600 dark:text-red-400">
            {loadError}
          </p>
        )}

        {isEmpty && (
          <p
            data-testid="snippets-empty-state"
            className="text-sm text-neutral-500 dark:text-neutral-400"
          >
            No snippets yet. Add one above.
          </p>
        )}

        {snippets !== null && snippets.length > 0 && (
          <ul className="flex flex-col gap-2" data-testid="snippets-list">
            {snippets.map((s) => {
              const draft = drafts[s.id] ?? { trigger: s.trigger, body: s.body };
              const busy = savingId === s.id || removingId === s.id;
              return (
                <li
                  key={s.id}
                  data-testid={`snippet-${s.id}`}
                  className="flex flex-col gap-2 rounded-md border border-neutral-200 px-3 py-2 dark:border-neutral-800"
                >
                  <div className="flex items-center gap-2">
                    <input
                      data-testid={`snippet-trigger-${s.id}`}
                      type="text"
                      value={draft.trigger}
                      disabled={busy}
                      onChange={(e) => handleEditFieldChange(s.id, "trigger", e.target.value)}
                      onBlur={() => commitEdit(s)}
                      className="min-w-0 flex-1 rounded-md border border-neutral-300 bg-white px-2 py-1 text-sm font-medium disabled:cursor-not-allowed disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-950"
                    />
                    <button
                      type="button"
                      data-testid={`snippet-remove-${s.id}`}
                      disabled={busy}
                      onClick={() => handleRemove(s.id)}
                      className="shrink-0 rounded-md border border-neutral-300 px-2 py-1 text-xs font-medium text-red-600 disabled:cursor-not-allowed disabled:opacity-50 hover:bg-red-50 dark:border-neutral-700 dark:text-red-400 dark:hover:bg-red-950/30"
                    >
                      {removingId === s.id ? "Removing…" : "Remove"}
                    </button>
                  </div>
                  <textarea
                    data-testid={`snippet-body-${s.id}`}
                    value={draft.body}
                    disabled={busy}
                    rows={2}
                    onChange={(e) => handleEditFieldChange(s.id, "body", e.target.value)}
                    onBlur={() => commitEdit(s)}
                    className="min-w-0 resize-y rounded-md border border-neutral-300 bg-white px-2 py-1 text-sm disabled:cursor-not-allowed disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-950"
                  />
                  {rowErrors[s.id] && (
                    <p
                      data-testid={`snippet-edit-error-${s.id}`}
                      className="text-xs text-red-600 dark:text-red-400"
                    >
                      {rowErrors[s.id]}
                    </p>
                  )}
                </li>
              );
            })}
          </ul>
        )}

        {removeError && (
          <p data-testid="snippets-remove-error" className="text-xs text-red-600 dark:text-red-400">
            {removeError}
          </p>
        )}
      </div>
    </div>
  );
}
