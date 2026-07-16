import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { invoke, type ToneProfile, type ToneRule } from "../../lib/ipc";

type LoadState = "loading" | "ready" | "error";

const TONE_OPTIONS: readonly ToneProfile[] = ["casual", "formal", "verbatim"];

/**
 * Every error string this component ever renders is one of these statics —
 * never a raw backend error message — mirroring the privacy rule that app
 * patterns (and, by extension, anything derived from a failed call touching
 * them) must never leak into a rendered string (MISSION §5/§7; see
 * `DictionaryTab.tsx`'s equivalent doc comment and PR #223's Sentinel
 * digest, issue #227, for the gap this avoids).
 */
const LOAD_ERROR_MESSAGE = "Couldn't load your tone rules. Try again.";
const ADD_ERROR_MESSAGE = "Couldn't add that rule. Try again.";
const REMOVE_ERROR_MESSAGE = "Couldn't remove that rule. Try again.";
const EDIT_ERROR_MESSAGE = "Couldn't update that rule. Try again.";
const EMPTY_INPUT_MESSAGE = "Enter an app pattern to add.";
const DUPLICATE_MESSAGE = "You already have a rule for that pattern — edit its tone below instead.";

function toneLabel(tone: ToneProfile): string {
  switch (tone) {
    case "casual":
      return "Casual";
    case "formal":
      return "Formal";
    case "verbatim":
      return "Verbatim";
  }
}

/**
 * Tone settings tab (issue #203, M3 PR 3.7): the user's per-app tone
 * overrides — an app-identifier glob pattern mapped to a tone profile
 * (casual/formal/verbatim), rendered as an ordered list with an add-rule
 * form and per-rule remove/edit. Talks to the core only through
 * `src/lib/ipc.ts`, per docs/ARCHITECTURE.md §Module Boundaries.
 *
 * ## Match order (AC-44/AC-45, PRD AC-22)
 *
 * `list_tone_rules` returns rows in insertion order (`id` ASC), which is
 * also `context::resolve_tone_for_app`'s first-match-wins walk order (see
 * that function's doc comment in src-tauri/src/context.rs) — this component
 * renders that order as-is (an `<ol>`, numbered) with an explanatory note,
 * so the list visually communicates match order. There is no reorder
 * command on the backend (PR #233 only ships list/upsert/delete), so
 * reordering isn't offered here; a rule's position is fixed by when it was
 * added. A newly added rule is the newest row (highest `id`) and so is
 * appended at the END of the rendered list, matching that same order —
 * deliberately NOT prepended like `DictionaryTab`'s newest-first list, since
 * here position is meaningful (match priority), not just recency.
 *
 * ## Add-rule validation
 *
 * `commands::upsert_tone_rule` is an upsert keyed on `app_pattern`
 * (case-insensitive): re-submitting an existing pattern UPDATES that rule's
 * tone in place rather than rejecting or adding a second row — the backend
 * never rejects a "duplicate" add. Appending a client-side row on that
 * response would therefore render a phantom second row that doesn't exist
 * server-side. So, mirroring `DictionaryTab.tsx`'s AC-39 pattern, a
 * case-insensitive duplicate of an already-loaded pattern is caught here,
 * client-side, against the loaded list, before the backend is ever called:
 * an inline error points the user at the existing rule's tone select
 * instead. A blank/whitespace-only submission is rejected the same way.
 *
 * ## Editing a rule's tone (AC-44)
 *
 * Each row's tone `<select>` calls `upsert_tone_rule` with that row's own
 * `app_pattern` immediately on change (no separate update-by-id command
 * exists, and none is needed — upserting the same pattern updates the same
 * row by the backend's own case-insensitive-unique contract). A per-row
 * generation counter (`editGenRef`) guards against two rapid, out-of-order
 * edits to the SAME rule: only the response matching the row's latest
 * request is applied, so a slow first response can't clobber a faster
 * second one. A failed edit reverts the select to the rule's last-known
 * good tone and shows a row-scoped inline error; the select (and that row's
 * remove button) are disabled while its own edit is in flight, and the
 * remove button is disabled while an edit to that row is in flight (and
 * vice versa) so the two mutations on one row can't overlap.
 *
 * Privacy (MISSION §5/§7): app patterns are user-environment data — this
 * component never `console.log`s a pattern, and every rendered error is one
 * of the static, kind-derived strings above, never a raw backend message.
 */
export function ToneTab() {
  const [rules, setRules] = useState<ToneRule[] | null>(null);
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [loadError, setLoadError] = useState<string | null>(null);

  const [draftPattern, setDraftPattern] = useState("");
  const [draftTone, setDraftTone] = useState<ToneProfile>("casual");
  const [adding, setAdding] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);

  const [removingId, setRemovingId] = useState<number | null>(null);
  const [removeError, setRemoveError] = useState<string | null>(null);

  const [savingToneId, setSavingToneId] = useState<number | null>(null);
  const [editErrorId, setEditErrorId] = useState<number | null>(null);

  const cancelledRef = useRef(false);
  // Per-rule monotonic generation counter for the tone-edit guard: bumped at
  // the START of each edit request to that row; a response is applied only
  // if it's still the LATEST generation minted for that row when it settles.
  const editGenRef = useRef<Map<number, number>>(new Map());

  useEffect(() => {
    cancelledRef.current = false;
    invoke("list_tone_rules")
      .then((rows) => {
        if (cancelledRef.current) return;
        setRules(rows);
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

  const handleDraftPatternChange = useCallback((value: string) => {
    setDraftPattern(value);
    setAddError((prev) => (prev ? null : prev));
  }, []);

  const handleAddSubmit = useCallback(
    (e: FormEvent) => {
      e.preventDefault();
      const trimmed = draftPattern.trim();

      if (trimmed === "") {
        setAddError(EMPTY_INPUT_MESSAGE);
        return;
      }
      const isDuplicate = (rules ?? []).some(
        (r) => r.app_pattern.toLowerCase() === trimmed.toLowerCase(),
      );
      if (isDuplicate) {
        setAddError(DUPLICATE_MESSAGE);
        return;
      }

      setAddError(null);
      setAdding(true);
      invoke("upsert_tone_rule", { app_pattern: trimmed, tone: draftTone })
        .then((id) => {
          if (cancelledRef.current) return;
          setRules((prev) => [
            ...(prev ?? []),
            { id, app_pattern: trimmed, tone: draftTone, created_at_ms: Date.now() },
          ]);
          setDraftPattern("");
        })
        .catch(() => {
          if (cancelledRef.current) return;
          setAddError(ADD_ERROR_MESSAGE);
        })
        .finally(() => {
          if (!cancelledRef.current) setAdding(false);
        });
    },
    [draftPattern, draftTone, rules],
  );

  const handleRemove = useCallback((id: number) => {
    setRemoveError(null);
    setRemovingId(id);
    invoke("delete_tone_rule", { id })
      .then(() => {
        if (cancelledRef.current) return;
        setRules((prev) => (prev ? prev.filter((r) => r.id !== id) : prev));
      })
      .catch(() => {
        if (cancelledRef.current) return;
        setRemoveError(REMOVE_ERROR_MESSAGE);
      })
      .finally(() => {
        if (!cancelledRef.current) setRemovingId(null);
      });
  }, []);

  const handleToneChange = useCallback(
    (rule: ToneRule, nextTone: ToneProfile) => {
      const previousTone = rule.tone;
      const generation = (editGenRef.current.get(rule.id) ?? 0) + 1;
      editGenRef.current.set(rule.id, generation);

      setEditErrorId((prev) => (prev === rule.id ? null : prev));
      setSavingToneId(rule.id);
      // Optimistic: reflect the change immediately (AC-44), reverted on
      // failure or superseded by a newer edit's own optimistic update.
      setRules((prev) =>
        prev ? prev.map((r) => (r.id === rule.id ? { ...r, tone: nextTone } : r)) : prev,
      );

      invoke("upsert_tone_rule", { app_pattern: rule.app_pattern, tone: nextTone })
        .then(() => {
          if (cancelledRef.current) return;
          // Stale response guard: a newer edit to this same row has already
          // started — its own optimistic update / eventual settle owns the
          // row's displayed value now.
          if (editGenRef.current.get(rule.id) !== generation) return;
          setSavingToneId((prev) => (prev === rule.id ? null : prev));
        })
        .catch(() => {
          if (cancelledRef.current) return;
          if (editGenRef.current.get(rule.id) !== generation) return;
          setRules((prev) =>
            prev ? prev.map((r) => (r.id === rule.id ? { ...r, tone: previousTone } : r)) : prev,
          );
          setEditErrorId(rule.id);
          setSavingToneId((prev) => (prev === rule.id ? null : prev));
        });
    },
    [],
  );

  const isLoading = loadState === "loading" && rules === null;
  const isEmpty = loadState === "ready" && rules !== null && rules.length === 0;

  return (
    <div className="flex max-w-lg flex-col gap-6" data-testid="tone-panel">
      <div className="flex flex-col gap-1">
        <span className="text-sm font-medium">Add a rule</span>
        <form className="flex items-center gap-2" onSubmit={handleAddSubmit}>
          <input
            data-testid="tone-add-pattern-input"
            type="text"
            value={draftPattern}
            placeholder="e.g. SynthMail, ChatSynth*"
            onChange={(e) => handleDraftPatternChange(e.target.value)}
            className="min-w-0 flex-1 rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:outline-none dark:border-neutral-700 dark:bg-neutral-950"
          />
          <select
            data-testid="tone-add-tone-select"
            value={draftTone}
            onChange={(e) => setDraftTone(e.target.value as ToneProfile)}
            className="shrink-0 rounded-md border border-neutral-300 bg-white px-2 py-2 text-sm dark:border-neutral-700 dark:bg-neutral-950"
          >
            {TONE_OPTIONS.map((tone) => (
              <option key={tone} value={tone}>
                {toneLabel(tone)}
              </option>
            ))}
          </select>
          <button
            type="submit"
            data-testid="tone-add-button"
            disabled={adding}
            className="shrink-0 rounded-md bg-blue-600 px-3 py-2 text-sm font-medium text-white disabled:cursor-not-allowed disabled:opacity-50 hover:bg-blue-500"
          >
            {adding ? "Adding…" : "Add"}
          </button>
        </form>
        <p className="text-xs text-neutral-500 dark:text-neutral-400">
          An app-name pattern (supports <code>*</code>/<code>?</code> wildcards,
          case-insensitive) and the tone bla should apply while dictating into that app.
        </p>
        {addError && (
          <p data-testid="tone-add-error" className="text-xs text-red-600 dark:text-red-400">
            {addError}
          </p>
        )}
      </div>

      <div className="flex flex-col gap-2 border-t border-neutral-200 pt-4 dark:border-neutral-800">
        {isLoading && (
          <p data-testid="tone-loading" className="text-sm text-neutral-500 dark:text-neutral-400">
            Loading…
          </p>
        )}

        {loadState === "error" && (
          <p data-testid="tone-load-error" className="text-xs text-red-600 dark:text-red-400">
            {loadError}
          </p>
        )}

        {isEmpty && (
          <p
            data-testid="tone-empty-state"
            className="text-sm text-neutral-500 dark:text-neutral-400"
          >
            No tone rules yet. Add one above.
          </p>
        )}

        {rules !== null && rules.length > 0 && (
          <>
            <p className="text-xs text-neutral-500 dark:text-neutral-400">
              Checked top to bottom — the first matching pattern wins.
            </p>
            <ol className="flex flex-col gap-1" data-testid="tone-list">
              {rules.map((r, index) => {
                const busy = savingToneId === r.id || removingId === r.id;
                return (
                  <li
                    key={r.id}
                    data-testid={`tone-rule-${r.id}`}
                    className="flex items-center justify-between gap-3 rounded-md border border-neutral-200 px-3 py-2 dark:border-neutral-800"
                  >
                    <span className="flex min-w-0 items-center gap-2">
                      <span className="shrink-0 text-xs tabular-nums text-neutral-400 dark:text-neutral-500">
                        {index + 1}.
                      </span>
                      <span
                        data-testid={`tone-rule-pattern-${r.id}`}
                        className="min-w-0 truncate text-sm text-neutral-900 dark:text-neutral-100"
                      >
                        {r.app_pattern}
                      </span>
                    </span>
                    <span className="flex shrink-0 items-center gap-2">
                      <select
                        data-testid={`tone-rule-tone-select-${r.id}`}
                        value={r.tone}
                        disabled={busy}
                        onChange={(e) => handleToneChange(r, e.target.value as ToneProfile)}
                        className="rounded-md border border-neutral-300 bg-white px-2 py-1 text-xs disabled:cursor-not-allowed disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-950"
                      >
                        {TONE_OPTIONS.map((tone) => (
                          <option key={tone} value={tone}>
                            {toneLabel(tone)}
                          </option>
                        ))}
                      </select>
                      <button
                        type="button"
                        data-testid={`tone-rule-remove-${r.id}`}
                        disabled={busy}
                        onClick={() => handleRemove(r.id)}
                        className="rounded-md border border-neutral-300 px-2 py-1 text-xs font-medium text-red-600 disabled:cursor-not-allowed disabled:opacity-50 hover:bg-red-50 dark:border-neutral-700 dark:text-red-400 dark:hover:bg-red-950/30"
                      >
                        {removingId === r.id ? "Removing…" : "Remove"}
                      </button>
                    </span>
                    {editErrorId === r.id && (
                      <p
                        data-testid={`tone-rule-edit-error-${r.id}`}
                        className="basis-full text-xs text-red-600 dark:text-red-400"
                      >
                        {EDIT_ERROR_MESSAGE}
                      </p>
                    )}
                  </li>
                );
              })}
            </ol>
          </>
        )}

        {removeError && (
          <p data-testid="tone-remove-error" className="text-xs text-red-600 dark:text-red-400">
            {removeError}
          </p>
        )}
      </div>
    </div>
  );
}
