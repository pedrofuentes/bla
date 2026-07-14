import { useCallback, useEffect, useRef, useState } from "react";
import {
  invoke,
  onEvent,
  type ModelPreset,
  type ModelRegistryEntry,
  type Settings,
} from "../../lib/ipc";
import {
  formatBytes,
  modelPresetLabel,
  modelStatusLabel,
  type ModelStatus,
} from "../../lib/status";
import { chordFromKeyboardEvent } from "../../lib/hotkeyChord";
import { applySettingsPatch, revertPatchedFields } from "../../lib/settingsPatch";

const MODEL_PRESETS: readonly ModelPreset[] = ["LargeV3Turbo", "Small"];

/**
 * Upper bound on how long an auto-apply's `set_settings` may take before it's
 * treated as failed (PR #185 cycle-4 🟡-3). Without this a hung IPC would
 * leave `applyInFlightRef` pinned above zero forever, silently disabling
 * hotkey capture (the gate) for the rest of the session; the timeout instead
 * rejects into the normal revert path.
 */
const SET_SETTINGS_TIMEOUT_MS = 15_000;

type SaveStatus = "idle" | "saving" | "saved";

/** Rejects if `promise` hasn't settled within `ms`; always clears its timer. */
function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout>;
  const timeout = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => reject(new Error(message)), ms);
  });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
}

/**
 * General settings tab (issue #126, M2 PR 2.5): hotkey capture, Whisper
 * model preset (with download progress reusing the pattern from `App.tsx`),
 * and hold-vs-toggle recording mode. Talks to the core only through
 * `src/lib/ipc.ts`.
 *
 * Issue #183 (AC-7 smoke test): every control here auto-applies on change —
 * there is no Save button. The cofounder changed the model preset, the
 * hold/toggle mode, and the hotkey in the AC-7 smoke test, saw nothing
 * happen (the previous flow required a separate Save click), and reported
 * all three as broken. The backend already applies everything live
 * (`commands::set_settings` -> `apply_settings_to_state`), so each control's
 * `onChange` calls `applySettingsChange`, showing a brief "Saved"
 * confirmation (`saveStatus`) or an inline `save-error` on failure. The
 * hotkey field is the one exception: issue #91's validate-before-persist
 * invariant still applies — a captured chord is validated first, and only a
 * chord that validates is auto-applied; an invalid one shows an inline error
 * and is never sent to `set_settings`.
 *
 * ## Concurrency model (PR #185 cycle-3 — holistic refactor)
 *
 * Two independent hazards — overlapping optimistic applies, and two owners
 * of the OS shortcut registration — caused a string of interleave bugs, so
 * both are eliminated by construction rather than patched:
 *
 * 1. **Serial apply queue.** `applySettingsChange` enqueues onto a single
 *    promise chain (`applyQueueRef`); exactly one apply runs at a time, each
 *    awaiting the previous one's `set_settings` before it starts. Because
 *    applies never overlap there is no lost update, no stale-closure merge,
 *    and no "a newer apply superseded this one" reconciliation: an apply
 *    reads the latest settings (`settingsRef`) when it *runs*, and on
 *    rejection it simply reverts `settingsRef`/state to the base it captured
 *    (no later apply has run yet). `applyInFlightRef` counts queued+running
 *    applies for the capture gate below.
 *
 * 2. **Single owner of the global shortcut.** Only the
 *    `suspend_hotkey`/`resume_hotkey` pair (backend, guarded by a monotonic
 *    generation token) ever registers/unregisters the dictation shortcut —
 *    `set_settings` no longer touches it. Focusing the field suspends
 *    (minting a generation); every way capture ends resumes with that same
 *    generation. A committed chord that CHANGED the hotkey persists via the
 *    queued `set_settings`, then resumes so the sole owner re-registers the
 *    newly-persisted chord (ordered after the save); an unchanged chord and
 *    the cancel/blur/invalid paths resume immediately. To keep a capture
 *    from ever racing a settings write, `beginCapture` is *gated* on
 *    `applyInFlightRef` — it won't start (won't suspend) while any apply is
 *    in flight, so the commit→refocus interleave can't occur.
 *
 * Safety nets so the shortcut can't be left dead: the effect cleanup resumes
 * if the component unmounts mid-capture (the hidden-not-destroyed settings
 * window is covered backend-side by `force_resume_hotkey` +
 * `hotkey-capture-reset`), and every suspend/resume invoke has a `.catch`
 * that surfaces an OS rejection as a save error. All async continuations are
 * guarded by `cancelledRef` so a late resolution after unmount is a no-op.
 *
 * Issue #184: the model picker shows each preset's download size (e.g.
 * "Small — 488 MB"), fetched from the `model_registry` command
 * (`ModelRegistryEntry[]`, mirroring `models::ModelSpec.size_bytes`) and
 * formatted with `formatBytes`. Falls back to the plain preset label if the
 * registry hasn't loaded (or failed to) yet.
 *
 * Event subscriptions (PR #134 Sentinel 🔴-1) are established individually,
 * not via a single `Promise.all`: a rejected subscription (the observable
 * shape of a capability/ACL misconfiguration — exactly what silently broke
 * this window when `capabilities/` only covered the main window) is
 * surfaced in the UI instead of vanishing as an unhandled rejection, and
 * the subscriptions that DID succeed keep their unlisten cleanup on
 * unmount. The window's own event access is granted by
 * `src-tauri/capabilities/settings.json`.
 *
 * The `settings` snapshot is mirrored in `settingsRef` so the serial apply
 * queue and the `output-mode-changed` subscription (PR #134 Sentinel 🔴-2,
 * mirroring `App.tsx`) read/merge against the latest value; a concurrent
 * tray-/status-window mode switch is therefore never clobbered by the next
 * auto-apply.
 */
export function GeneralTab() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [hotkeyInput, setHotkeyInput] = useState("");
  const [hotkeyError, setHotkeyError] = useState<string | null>(null);
  const [capturing, setCapturing] = useState(false);
  const [modelStatus, setModelStatus] = useState<ModelStatus>("checking");
  const [downloadPercent, setDownloadPercent] = useState<number | undefined>(undefined);
  const [modelRegistry, setModelRegistry] = useState<ModelRegistryEntry[]>([]);
  const [saveStatus, setSaveStatus] = useState<SaveStatus>("idle");
  const [saveError, setSaveError] = useState<string | null>(null);
  const [eventsError, setEventsError] = useState<string | null>(null);

  // The latest known settings, read by each queued apply when it RUNS (see
  // the concurrency-model doc comment). Kept in lock-step with `settings`.
  const settingsRef = useRef<Settings | null>(null);
  // Serial apply queue + in-flight count. `applyQueueRef` chains applies so
  // exactly one runs at a time; `applyInFlightRef` (queued + running) is the
  // signal `beginCapture` gates on so a capture never races a settings write.
  const applyQueueRef = useRef<Promise<void>>(Promise.resolve());
  const applyInFlightRef = useRef(0);
  // The generation of the current outstanding hotkey-capture suspend (null
  // when not suspended) + the monotonic counter that mints them.
  // `resume_hotkey` echoes the active generation so the backend rejects a
  // stale, out-of-order resume.
  const suspendGenCounterRef = useRef(0);
  const activeSuspendGenRef = useRef<number | null>(null);
  // Set on unmount so late async continuations (queued applies, suspend/
  // resume catches, the model-download re-check) no-op instead of touching
  // state on an unmounted tree.
  const cancelledRef = useRef(false);

  useEffect(() => {
    let cancelled = false;

    invoke("get_settings")
      .then((loaded) => {
        if (cancelled) return;
        settingsRef.current = loaded;
        setSettings(loaded);
        setHotkeyInput(loaded.hotkey);
      })
      .catch((err) => {
        if (!cancelled) setSaveError(String(err));
      });

    invoke("download_selected_model")
      .then((result) => {
        if (cancelled) return;
        setModelStatus(result === "already-present" ? "ready" : "downloading");
      })
      .catch((err) => {
        if (!cancelled) {
          setModelStatus("error");
          setSaveError(String(err));
        }
      });

    // Issue #184: best-effort — a failed fetch just leaves the picker
    // showing plain preset labels (no size suffix) rather than blocking or
    // erroring the whole tab over a non-critical enhancement.
    invoke("model_registry")
      .then((entries) => {
        if (!cancelled) setModelRegistry(entries);
      })
      .catch(() => {
        /* picker falls back to plain labels; nothing else depends on this */
      });

    // PR #134 Sentinel 🔴-1: NOT a single Promise.all — one rejected
    // subscription must neither hide the failure (it's surfaced via
    // eventsError) nor discard the unlisten cleanup of the subscriptions
    // that succeeded.
    const active: Array<() => void> = [];
    const subscriptions: Array<Promise<() => void>> = [
      onEvent("model-download-progress", (progress) => {
        if (cancelled) return;
        setModelStatus("downloading");
        setDownloadPercent(progress.percent);
      }),
      onEvent("model-download-complete", () => {
        if (cancelled) return;
        setModelStatus("ready");
        setDownloadPercent(undefined);
      }),
      onEvent("model-download-error", (message) => {
        if (!cancelled) {
          setModelStatus("error");
          setSaveError(message);
        }
      }),
      // PR #134 Sentinel 🔴-2 (mirrors App.tsx): keep the snapshot every
      // auto-apply merges onto in sync with tray-/status-window-triggered
      // mode switches — updating the ref (PR #185 🔴-2) as well as state so
      // the next auto-apply doesn't clobber the concurrent change.
      onEvent("output-mode-changed", (mode) => {
        if (cancelled) return;
        const base = settingsRef.current;
        if (!base) return;
        const nextSettings = { ...base, output_mode: mode };
        settingsRef.current = nextSettings;
        setSettings(nextSettings);
      }),
      // PR #185 Sentinel delta 🟡-3: the window was hidden mid-capture (it
      // hides, not unmounts, so the effect-cleanup reset below never ran).
      // The backend already force-restored the OS shortcut; drop out of
      // capture mode and clear the stale suspend so the field isn't stuck
      // swallowing keys when reopened.
      onEvent("hotkey-capture-reset", () => {
        if (cancelled) return;
        activeSuspendGenRef.current = null;
        setCapturing(false);
        setHotkeyInput(settingsRef.current?.hotkey ?? "");
      }),
    ];
    for (const subscription of subscriptions) {
      subscription
        .then((unlisten) => {
          // Resolved after unmount: unsubscribe immediately instead of
          // pushing to a list nobody will drain.
          if (cancelled) unlisten();
          else active.push(unlisten);
        })
        .catch((err) => {
          if (!cancelled) setEventsError(String(err));
        });
    }

    return () => {
      cancelled = true;
      cancelledRef.current = true;
      for (const unlisten of active) unlisten();
      // PR #185 🔴-1(b): if the component unmounts while a capture suspend is
      // still outstanding, restore the global shortcut so it can't be left
      // dead. (The settings window is hidden — not destroyed — on close, so
      // this rarely fires on a real close; that path is covered backend-side
      // by `force_resume_hotkey` + the `hotkey-capture-reset` event.)
      const pendingGen = activeSuspendGenRef.current;
      if (pendingGen !== null) {
        activeSuspendGenRef.current = null;
        void invoke("resume_hotkey", { generation: pendingGen }).catch(() => {
          /* unmounting — nothing left to surface the error on */
        });
      }
    };
  }, []);

  // Issue #181: unregister the global shortcut and mint a fresh generation
  // as the current outstanding suspend. Surfaces an OS rejection instead of
  // swallowing it (guarded so a post-unmount rejection is a no-op).
  const suspendHotkey = useCallback(() => {
    const generation = ++suspendGenCounterRef.current;
    activeSuspendGenRef.current = generation;
    void invoke("suspend_hotkey", { generation }).catch((err) => {
      if (!cancelledRef.current) setSaveError(String(err));
    });
  }, []);

  // Issue #181: restore the shortcut, echoing the active suspend's generation
  // so the backend rejects it if a newer suspend has since superseded it.
  // No-op if there's no outstanding suspend. The SINGLE code path (with the
  // backend) that re-registers the shortcut after a capture.
  const resumeHotkey = useCallback(() => {
    const generation = activeSuspendGenRef.current;
    activeSuspendGenRef.current = null;
    if (generation === null) return;
    void invoke("resume_hotkey", { generation }).catch((err) => {
      if (!cancelledRef.current) setSaveError(String(err));
    });
  }, []);

  // The body of one queued apply (see the concurrency-model doc comment).
  // Runs serially — reads `settingsRef` when it STARTS (up to date, since no
  // other apply overlaps), so on rejection it can simply revert to the base
  // it captured. Never throws (the queue keeps draining).
  const runApply = useCallback(
    async (patch: Partial<Settings>) => {
      const base = settingsRef.current;
      if (!base) return;
      const next = applySettingsPatch(base, patch);
      settingsRef.current = next;
      setSettings(next);
      setSaveStatus("saving");
      setSaveError(null);

      try {
        // 🟡-3: a hung set_settings must not pin the in-flight gate forever.
        await withTimeout(
          invoke("set_settings", { settings: next }),
          SET_SETTINGS_TIMEOUT_MS,
          "Saving timed out",
        );
        if (cancelledRef.current) return;
        setSaveStatus("saved");
        // 🔴-1 single registration point: a committed CHANGED hotkey is bound
        // by set_settings itself (its register-before-persist gate) and the
        // backend cleared the suspend generation — so do NOT resume (that
        // would double-register). Consume the frontend suspend so the unmount
        // net doesn't resume either.
        if (patch.hotkey !== undefined) {
          activeSuspendGenRef.current = null;
        }
        // Issue #91/#110 pattern reuse: after a preset change auto-applies,
        // re-check the newly-selected preset's on-disk status.
        if (patch.model_preset !== undefined) {
          invoke("download_selected_model")
            .then((result) => {
              if (!cancelledRef.current) {
                setModelStatus(result === "already-present" ? "ready" : "downloading");
              }
            })
            .catch((err) => {
              if (!cancelledRef.current) {
                setModelStatus("error");
                setSaveError(String(err));
              }
            });
        }
      } catch (err) {
        if (cancelledRef.current) return;
        setSaveStatus("idle");
        setSaveError(String(err));
        // 🔴-2: revert ONLY the field(s) THIS apply patched, back onto the
        // CURRENT settings — an out-of-band `output-mode-changed` write that
        // landed while we were in flight must survive, not be clobbered by a
        // blind revert-to-base.
        const current = settingsRef.current ?? base;
        const reverted = revertPatchedFields(current, base, patch);
        settingsRef.current = reverted;
        setSettings(reverted);
        if (patch.hotkey !== undefined) {
          setHotkeyInput(reverted.hotkey);
          // 🔴-1: set_settings's register-before-persist unregistered the old
          // chord before failing to bind the new one, so the shortcut is
          // currently unbound — restore the prior binding (resume registers
          // the persisted, i.e. old, hotkey) so it can't be left dead.
          resumeHotkey();
        }
      }
    },
    [resumeHotkey],
  );

  // Issue #183: every control's auto-apply enqueues onto the serial queue.
  // `applyInFlightRef` (queued + running) is the capture gate's signal.
  const applySettingsChange = useCallback(
    (patch: Partial<Settings>) => {
      applyInFlightRef.current += 1;
      const task = () =>
        runApply(patch).finally(() => {
          applyInFlightRef.current -= 1;
        });
      // `task` never rejects (runApply catches), so the chain stays alive;
      // the second arg keeps it draining even if a prior link somehow did.
      applyQueueRef.current = applyQueueRef.current.then(task, task);
      return applyQueueRef.current;
    },
    [runApply],
  );

  // Issue #181: ends hotkey capture, optionally reverting the field's
  // displayed value, and always restoring the global shortcut (resume is the
  // single owner). Used for the cancel/blur/invalid paths; a committed chord
  // is handled inline in `handleHotkeyKeyDown` (its changed variant resumes
  // only after the queued save persists the new binding).
  const endCapture = useCallback(
    (revertTo?: string) => {
      setCapturing(false);
      if (revertTo !== undefined) setHotkeyInput(revertTo);
      resumeHotkey();
    },
    [resumeHotkey],
  );

  const beginCapture = useCallback(() => {
    // Gate (PR #185 cycle-3): never start a capture — and never suspend the
    // shortcut — while a settings apply is in flight, so a capture can't race
    // a concurrent settings write (the commit→refocus / apply-during-save
    // interleave). The blocked focus simply doesn't enter capture mode.
    if (applyInFlightRef.current > 0) return;
    setCapturing(true);
    setHotkeyError(null);
    suspendHotkey();
  }, [suspendHotkey]);

  const handleHotkeyBlur = useCallback(() => {
    // Only a genuine "lost focus mid-capture" needs a resume — a blur firing
    // right after a chord already committed (capturing is already false by
    // then) must not resume against a save that may still be in flight.
    if (capturing) {
      endCapture(settingsRef.current?.hotkey ?? hotkeyInput);
    }
  }, [capturing, hotkeyInput, endCapture]);

  const handleHotkeyKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (!capturing) return;
      // Swallow the keydown's default action (typing/Tab/etc.) only while
      // actively capturing a chord — a read-only field with no capture in
      // progress should still let Tab move focus normally.
      e.preventDefault();

      if (e.key === "Escape") {
        endCapture(settingsRef.current?.hotkey ?? hotkeyInput);
        return;
      }

      const chord = chordFromKeyboardEvent(e.nativeEvent);
      if (chord === null) {
        // Bare modifier, or a main key with no modifier held yet — keep
        // listening for more keys rather than treating this as a cancel.
        return;
      }

      setCapturing(false);
      setHotkeyInput(chord);

      // 🟡-4: hold the in-flight gate SYNCHRONOUSLY, from the moment capture
      // ends — before the `validate_hotkey` round-trip — so a refocus during
      // that async gap can't slip past `beginCapture` and mint a second
      // suspend. Released in every branch below (the changed path hands the
      // hold off to the queued apply so the gate stays continuous).
      applyInFlightRef.current += 1;
      let released = false;
      const releaseCommitHold = () => {
        if (!released) {
          released = true;
          applyInFlightRef.current -= 1;
        }
      };

      invoke("validate_hotkey", { accelerator: chord })
        .then(() => {
          if (cancelledRef.current) {
            releaseCommitHold();
            return;
          }
          setHotkeyError(null);
          const changed = chord !== (settingsRef.current?.hotkey ?? "");
          if (changed) {
            // Persist via the serial queue; set_settings binds the new chord
            // (its register-before-persist gate) — don't resume here. Keep the
            // gate held until that apply settles so the window stays covered.
            applySettingsChange({ hotkey: chord }).finally(releaseCommitHold);
          } else {
            // Unchanged chord: nothing to persist — just restore the shortcut.
            resumeHotkey();
            releaseCommitHold();
          }
        })
        .catch((err) => {
          if (!cancelledRef.current) {
            setHotkeyError(String(err));
            // Invalid: keep the rejected chord + error visible; resume only.
            endCapture();
          }
          releaseCommitHold();
        });
    },
    [capturing, hotkeyInput, endCapture, applySettingsChange, resumeHotkey],
  );

  if (!settings) {
    return <p className="text-sm text-neutral-500 dark:text-neutral-400">Loading…</p>;
  }

  return (
    <div className="flex max-w-md flex-col gap-6" data-testid="general-panel">
      <div className="flex flex-col gap-1">
        <label htmlFor="hotkey-input" className="text-sm font-medium">
          Hotkey
        </label>
        <input
          id="hotkey-input"
          data-testid="hotkey-input"
          type="text"
          readOnly
          value={capturing ? "Press a key combination… (Esc to cancel)" : hotkeyInput}
          onFocus={beginCapture}
          onBlur={handleHotkeyBlur}
          onKeyDown={handleHotkeyKeyDown}
          className="rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:outline-none dark:border-neutral-700 dark:bg-neutral-950"
        />
        {hotkeyError && (
          <p data-testid="hotkey-error" className="text-xs text-red-600 dark:text-red-400">
            {hotkeyError}
          </p>
        )}
      </div>

      <fieldset className="flex flex-col gap-2">
        <legend className="text-sm font-medium">Recording mode</legend>
        <div className="flex gap-4 text-sm">
          <label className="flex items-center gap-2">
            <input
              type="radio"
              name="recording-mode"
              data-testid="mode-hold"
              checked={settings.recording_mode === "Hold"}
              onChange={() => void applySettingsChange({ recording_mode: "Hold" })}
            />
            Hold to record
          </label>
          <label className="flex items-center gap-2">
            <input
              type="radio"
              name="recording-mode"
              data-testid="mode-toggle"
              checked={settings.recording_mode === "Toggle"}
              onChange={() => void applySettingsChange({ recording_mode: "Toggle" })}
            />
            Toggle to record
          </label>
        </div>
      </fieldset>

      <div className="flex flex-col gap-1">
        <label htmlFor="model-preset-select" className="text-sm font-medium">
          Model
        </label>
        <select
          id="model-preset-select"
          data-testid="model-preset-select"
          value={settings.model_preset}
          onChange={(e) =>
            void applySettingsChange({ model_preset: e.target.value as ModelPreset })
          }
          className="rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm dark:border-neutral-700 dark:bg-neutral-950"
        >
          {MODEL_PRESETS.map((preset) => {
            const sizeBytes = modelRegistry.find((entry) => entry.preset === preset)?.size_bytes;
            const label =
              sizeBytes === undefined
                ? modelPresetLabel(preset)
                : `${modelPresetLabel(preset)} — ${formatBytes(sizeBytes)}`;
            return (
              <option key={preset} value={preset}>
                {label}
              </option>
            );
          })}
        </select>
        <p data-testid="model-status" className="text-xs text-neutral-500 dark:text-neutral-400">
          {modelStatusLabel(modelStatus, downloadPercent)}
        </p>
      </div>

      <div className="flex flex-col gap-2">
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            data-testid="launch-at-login-checkbox"
            checked={settings.launch_at_login}
            onChange={(e) => void applySettingsChange({ launch_at_login: e.target.checked })}
          />
          Launch bla at login
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            data-testid="sound-cues-checkbox"
            checked={settings.sound_cues}
            onChange={(e) => void applySettingsChange({ sound_cues: e.target.checked })}
          />
          Play sound cues
        </label>
      </div>

      {eventsError && (
        <p data-testid="events-error" className="text-xs text-red-600 dark:text-red-400">
          Live status updates are unavailable: {eventsError}
        </p>
      )}

      {saveError && (
        <p data-testid="save-error" className="text-xs text-red-600 dark:text-red-400">
          {saveError}
        </p>
      )}

      {saveStatus === "saved" && (
        <span data-testid="save-status" className="text-xs text-neutral-500 dark:text-neutral-400">
          Saved ✓
        </span>
      )}
    </div>
  );
}
