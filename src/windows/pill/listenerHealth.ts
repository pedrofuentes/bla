/**
 * Pure per-listener failure tracking for the pill's event subscriptions
 * (issue #182). Extracted out of `index.tsx` so the "how does a rejection
 * or a later success change the failed set, and when does that add up to
 * every listener being down" decision is independently unit-tested, the
 * same pure-decision/thin-glue split `soundCue.ts`/`toast.ts` already use
 * elsewhere in this window -- `index.tsx` only calls these from its
 * `onEvent(...).then/.catch` handlers.
 *
 * A `Set`, not a single sticky boolean, is the fix itself: the pre-#182
 * bug was one `eventsError` flag set on ANY rejection and never cleared,
 * so a single failed `audio-level` subscription blanked the whole pill
 * (including the state dot fed by a perfectly working
 * `pipeline-state-changed` listener) and stayed blanked even after other
 * subscriptions succeeded.
 */

/** The three backend events the pill subscribes to. */
export type ListenerName = "pipeline-state-changed" | "audio-level" | "pipeline-error";

export const ALL_LISTENERS: readonly ListenerName[] = [
  "pipeline-state-changed",
  "audio-level",
  "pipeline-error",
];

/** Returns a new set with `name` marked failed (returns `failed` unchanged if already present). */
export function withListenerFailed(
  failed: ReadonlySet<ListenerName>,
  name: ListenerName,
): ReadonlySet<ListenerName> {
  return failed.has(name) ? failed : new Set(failed).add(name);
}

/**
 * Returns a new set with `name`'s failure cleared (returns `failed`
 * unchanged if it wasn't marked failed) -- called from a subscription's
 * successful resolution so a listener's own success always wins over its
 * own prior rejection, regardless of settle order relative to the other
 * two listeners.
 */
export function withListenerRecovered(
  failed: ReadonlySet<ListenerName>,
  name: ListenerName,
): ReadonlySet<ListenerName> {
  if (!failed.has(name)) return failed;
  const next = new Set(failed);
  next.delete(name);
  return next;
}

/**
 * True once every one of {@link ALL_LISTENERS} is in `failed` -- the only
 * condition that should blank the whole pill to "Status unavailable"; any
 * lesser combination degrades just the feature(s) the failed listener(s)
 * feed (see `index.tsx`'s render).
 */
export function allListenersFailed(failed: ReadonlySet<ListenerName>): boolean {
  return ALL_LISTENERS.every((name) => failed.has(name));
}
