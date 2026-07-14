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
 * leave a settings write pending forever; the timeout instead rejects into
 * the normal revert path.
 */
const SET_SETTINGS_TIMEOUT_MS = 15_000;
const SET_SETTINGS_TIMEOUT_MESSAGE = "Saving timed out";
/** Upper bound on the capture-time `validate_hotkey` probe (PR #185 cycle-4 🟡). */
const VALIDATE_TIMEOUT_MS = 5_000;
const VALIDATE_TIMEOUT_MESSAGE = "Validating the hotkey timed out";

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
 * General settings tab (issue #126, M2 PR 2.5): hotkey binding, Whisper model
 * preset (with download progress), and hold-vs-toggle recording mode. Talks
 * to the core only through `src/lib/ipc.ts`.
 *
 * ## Two apply models
 *
 * Issue #183 (AC-7 smoke test): the model preset, recording mode,
 * launch-at-login and sound-cues controls **auto-apply on change** — each
 * `onChange` calls `applySettingsChange`, which enqueues a `set_settings`
 * onto a single serial queue (`applyQueueRef`, exactly one apply in flight at
 * a time, each built on the prior's result — no lost updates). A brief
 * "Saved" confirmation or an inline `save-error` follows; a failed apply
 * reverts only the field(s) it patched (`revertPatchedFields`, PR #185 🔴-2)
 * so a concurrent out-of-band `output-mode-changed` write survives.
 *
 * Issue #181 + #187 (cofounder DECISION): the **hotkey field uses an explicit
 * Apply button**, not auto-apply — this dissolves the capture-vs-apply
 * concurrency that caused a run of near-misses. The flow:
 *
 * 1. **Capture** (keystroke grabbing only): focusing the field enters capture
 *    and `suspend_hotkey`s the global dictation shortcut (minting a monotonic
 *    generation) so the user's keypresses reach the field instead of firing a
 *    dictation. A captured chord is shown as a **pending** value — it is NOT
 *    validated-for-persist, registered, or saved. Capture then ENDS (Escape,
 *    blur, or a chord captured) and `resume_hotkey`s the OLD, still-current
 *    hotkey, since nothing was persisted. The generation lets the backend
 *    reject a stale, out-of-order resume.
 * 2. **Apply**: the Apply button (enabled only for a valid pending chord that
 *    differs from the current hotkey) enqueues the hotkey change onto the
 *    SAME serial queue as the other controls. Its `set_settings` does the real
 *    register-before-persist (with rollback) on the core side; success clears
 *    the pending value, failure shows an inline error and leaves the old
 *    hotkey bound. Because capture has fully ended (old hotkey resumed) before
 *    Apply runs, `set_settings` is the sole registrar during Apply and never
 *    races the capture suspend/resume.
 *
 * Safety nets: the effect cleanup resumes if the component unmounts
 * mid-capture (the hidden-not-destroyed settings window is covered
 * backend-side by `force_resume_hotkey` + the `hotkey-capture-reset` event),
 * every suspend/resume/validate/set_settings call is time-bounded or
 * `.catch`-guarded, and all async continuations check `cancelledRef`. A
 * `set_settings` that times out but later succeeds is reconciled from
 * `get_settings` (PR #185 🟡) so the UI can't diverge from persisted truth.
 *
 * Issue #184: the model picker shows each preset's download size (e.g.
 * "Small — 488 MB") from `model_registry`, formatted with `formatBytes`.
 *
 * Event subscriptions (PR #134 Sentinel 🔴-1) are established individually,
 * not via a single `Promise.all`: a rejected subscription is surfaced in the
 * UI instead of vanishing, and the subscriptions that DID succeed keep their
 * unlisten cleanup. The `settings` snapshot is mirrored in `settingsRef` so
 * the serial queue and the `output-mode-changed` subscription read/merge the
 * latest value.
 */
export function GeneralTab() {
  const [settings, setSettings] = useState<Settings | null>(null);
  // The captured-but-not-yet-applied hotkey chord (`null` when there's no
  // pending change). Distinct from the persisted `settings.hotkey`.
  const [pendingHotkey, setPendingHotkey] = useState<string | null>(null);
  const [hotkeyError, setHotkeyError] = useState<string | null>(null);
  const [capturing, setCapturing] = useState(false);
  const [modelStatus, setModelStatus] = useState<ModelStatus>("checking");
  const [downloadPercent, setDownloadPercent] = useState<number | undefined>(undefined);
  const [modelRegistry, setModelRegistry] = useState<ModelRegistryEntry[]>([]);
  const [saveStatus, setSaveStatus] = useState<SaveStatus>("idle");
  const [saveError, setSaveError] = useState<string | null>(null);
  const [eventsError, setEventsError] = useState<string | null>(null);

  // The latest known settings, read by each queued apply when it RUNS.
  const settingsRef = useRef<Settings | null>(null);
  // Serial apply queue: chains applies so exactly one runs at a time.
  const applyQueueRef = useRef<Promise<void>>(Promise.resolve());
  // The generation of the current outstanding hotkey-capture suspend (null
  // when not suspended) + the monotonic counter that mints them.
  const suspendGenCounterRef = useRef(0);
  const activeSuspendGenRef = useRef<number | null>(null);
  // Synchronous mirror of `capturing` (state updates are async) so the blur
  // handler can tell a "still capturing → cancel" blur from the programmatic
  // blur we fire right after capturing a chord. Also the field ref used to
  // drop focus after a chord so re-focusing re-enters capture.
  const capturingRef = useRef(false);
  const hotkeyInputRef = useRef<HTMLInputElement>(null);
  // Set on unmount so late async continuations no-op instead of touching
  // state on an unmounted tree.
  const cancelledRef = useRef(false);

  useEffect(() => {
    let cancelled = false;

    invoke("get_settings")
      .then((loaded) => {
        if (cancelled) return;
        settingsRef.current = loaded;
        setSettings(loaded);
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

    // Issue #184: best-effort — a failed fetch just leaves the picker showing
    // plain preset labels rather than erroring the whole tab.
    invoke("model_registry")
      .then((entries) => {
        if (!cancelled) setModelRegistry(entries);
      })
      .catch(() => {
        /* picker falls back to plain labels; nothing else depends on this */
      });

    // PR #134 Sentinel 🔴-1: NOT a single Promise.all — one rejected
    // subscription must neither hide the failure (surfaced via eventsError)
    // nor discard the unlisten cleanup of the ones that succeeded.
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
      // PR #134 Sentinel 🔴-2 (mirrors App.tsx): keep the snapshot every apply
      // merges onto in sync with tray-/status-window-triggered mode switches —
      // updating the ref as well as state so the next apply doesn't clobber it.
      onEvent("output-mode-changed", (mode) => {
        if (cancelled) return;
        const base = settingsRef.current;
        if (!base) return;
        const nextSettings = { ...base, output_mode: mode };
        settingsRef.current = nextSettings;
        setSettings(nextSettings);
      }),
      // PR #185 delta 🟡-3: the window was hidden mid-capture (it hides, not
      // unmounts). The backend force-restored the OS shortcut; drop out of
      // capture mode and discard any pending chord so the field isn't stuck.
      onEvent("hotkey-capture-reset", () => {
        if (cancelled) return;
        activeSuspendGenRef.current = null;
        capturingRef.current = false;
        setCapturing(false);
        setPendingHotkey(null);
        setHotkeyError(null);
      }),
    ];
    for (const subscription of subscriptions) {
      subscription
        .then((unlisten) => {
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
      // dead. (The settings window HIDES on close — that path is covered
      // backend-side by `force_resume_hotkey` + `hotkey-capture-reset`.)
      const pendingGen = activeSuspendGenRef.current;
      if (pendingGen !== null) {
        activeSuspendGenRef.current = null;
        void invoke("resume_hotkey", { generation: pendingGen }).catch(() => {
          /* unmounting — nothing left to surface the error on */
        });
      }
    };
  }, []);

  // Issue #181: unregister the global shortcut and mint a fresh generation as
  // the current outstanding suspend. Surfaces an OS rejection (guarded so a
  // post-unmount rejection is a no-op).
  const suspendHotkey = useCallback(() => {
    const generation = ++suspendGenCounterRef.current;
    activeSuspendGenRef.current = generation;
    void invoke("suspend_hotkey", { generation }).catch((err) => {
      if (!cancelledRef.current) setSaveError(String(err));
    });
  }, []);

  // Issue #181: restore the shortcut, echoing the active suspend's generation
  // so the backend rejects it if a newer suspend superseded it. No-op if
  // there's no outstanding suspend. This is the ONLY registrar during capture
  // (Apply's set_settings is the registrar for a persisted change).
  const resumeHotkey = useCallback(() => {
    const generation = activeSuspendGenRef.current;
    activeSuspendGenRef.current = null;
    if (generation === null) return;
    void invoke("resume_hotkey", { generation }).catch((err) => {
      if (!cancelledRef.current) setSaveError(String(err));
    });
  }, []);

  // Resync UI state from persisted truth (PR #185 🟡): used when a timed-out
  // set_settings later SUCCEEDS on the backend, so the reverted UI can't
  // diverge from what actually persisted.
  const reconcileFromBackend = useCallback(() => {
    invoke("get_settings")
      .then((loaded) => {
        if (cancelledRef.current) return;
        settingsRef.current = loaded;
        setSettings(loaded);
        setPendingHotkey(null);
        setHotkeyError(null);
      })
      .catch((err) => {
        if (!cancelledRef.current) setSaveError(String(err));
      });
  }, []);

  // The body of one queued apply. Runs serially — reads `settingsRef` when it
  // STARTS (up to date, since no other apply overlaps), so on rejection it
  // reverts only the field(s) it patched. Never throws (the queue keeps
  // draining).
  const runApply = useCallback(
    async (patch: Partial<Settings>) => {
      const base = settingsRef.current;
      if (!base) return;
      const next = applySettingsPatch(base, patch);
      settingsRef.current = next;
      setSettings(next);
      setSaveStatus("saving");
      setSaveError(null);
      if (patch.hotkey !== undefined) setHotkeyError(null);

      const setPromise = invoke("set_settings", { settings: next });
      try {
        await withTimeout(setPromise, SET_SETTINGS_TIMEOUT_MS, SET_SETTINGS_TIMEOUT_MESSAGE);
        if (cancelledRef.current) return;
        setSaveStatus("saved");
        // Applied: the pending hotkey (if any) is now the persisted current.
        if (patch.hotkey !== undefined) {
          setPendingHotkey(null);
          setHotkeyError(null);
        }
        // Issue #91/#110 pattern reuse: after a preset change, re-check the
        // newly-selected preset's on-disk status.
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
        const message = String(err);
        setSaveError(message);
        // 🔴-2: revert only the field(s) THIS apply patched, onto the CURRENT
        // settings — an out-of-band output-mode change survives.
        const current = settingsRef.current ?? base;
        settingsRef.current = revertPatchedFields(current, base, patch);
        setSettings(settingsRef.current);
        if (patch.hotkey !== undefined) {
          // The register-before-persist gate on the core side rolled the OS
          // binding back to the prior hotkey; drop the pending value and show
          // the failure inline. The old hotkey stays bound.
          setPendingHotkey(null);
          setHotkeyError(message);
        }
        // 🟡 late-reconcile: a TIMED-OUT set_settings that later SUCCEEDS means
        // the backend persisted `next` while we reverted — resync from truth.
        if (err instanceof Error && err.message === SET_SETTINGS_TIMEOUT_MESSAGE) {
          setPromise.then(
            () => {
              if (!cancelledRef.current) reconcileFromBackend();
            },
            () => {
              /* a genuine failure: the revert already reflects it */
            },
          );
        }
      }
    },
    [reconcileFromBackend],
  );

  // Issue #183: every auto-apply (and the hotkey Apply button) enqueues onto
  // the serial queue.
  const applySettingsChange = useCallback(
    (patch: Partial<Settings>) => {
      const task = () => runApply(patch);
      // `task` never rejects (runApply catches), so the chain stays alive; the
      // second arg keeps it draining even if a prior link somehow did.
      applyQueueRef.current = applyQueueRef.current.then(task, task);
      return applyQueueRef.current;
    },
    [runApply],
  );

  // ---- Hotkey capture (keystroke grabbing only — never persists) ----

  const beginCapture = useCallback(() => {
    if (capturingRef.current) return; // already capturing (e.g. ref.focus re-entry)
    capturingRef.current = true;
    setCapturing(true);
    setPendingHotkey(null);
    setHotkeyError(null);
    suspendHotkey();
  }, [suspendHotkey]);

  // Ends capture WITHOUT keeping a pending chord (Escape / blur mid-capture):
  // restore the old (still-current) hotkey.
  const cancelCapture = useCallback(() => {
    capturingRef.current = false;
    setCapturing(false);
    setPendingHotkey(null);
    setHotkeyError(null);
    resumeHotkey();
  }, [resumeHotkey]);

  const handleHotkeyBlur = useCallback(() => {
    // A blur while actively capturing (no chord yet) cancels; a blur AFTER a
    // chord was captured — including the programmatic blur below — keeps the
    // pending value (capturingRef is already false by then).
    if (capturingRef.current) cancelCapture();
  }, [cancelCapture]);

  const handleHotkeyKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (!capturingRef.current) return;
      // Swallow the keydown's default only while actively capturing.
      e.preventDefault();

      if (e.key === "Escape") {
        cancelCapture();
        return;
      }

      const chord = chordFromKeyboardEvent(e.nativeEvent);
      if (chord === null) {
        // Bare modifier / no modifier yet — keep listening.
        return;
      }

      // Chord captured → END capture (restore the OLD hotkey; nothing is
      // persisted here) and show the chord as pending. Drop focus so
      // re-focusing the field re-enters capture. A parse probe drives the
      // inline error + Apply-button enablement; the authoritative
      // validate+register happens on Apply's set_settings.
      capturingRef.current = false;
      setCapturing(false);
      setPendingHotkey(chord);
      resumeHotkey();
      hotkeyInputRef.current?.blur();
      withTimeout(
        invoke("validate_hotkey", { accelerator: chord }),
        VALIDATE_TIMEOUT_MS,
        VALIDATE_TIMEOUT_MESSAGE,
      )
        .then(() => {
          if (!cancelledRef.current) setHotkeyError(null);
        })
        .catch((err) => {
          if (!cancelledRef.current) setHotkeyError(String(err));
        });
    },
    [cancelCapture, resumeHotkey],
  );

  const canApplyHotkey =
    !capturing &&
    pendingHotkey !== null &&
    pendingHotkey !== settings?.hotkey &&
    hotkeyError === null;

  const handleApplyHotkey = useCallback(() => {
    const chord = pendingHotkey;
    const current = settingsRef.current;
    if (chord === null || current === null || chord === current.hotkey || hotkeyError !== null) {
      return;
    }
    void applySettingsChange({ hotkey: chord });
  }, [pendingHotkey, hotkeyError, applySettingsChange]);

  if (!settings) {
    return <p className="text-sm text-neutral-500 dark:text-neutral-400">Loading…</p>;
  }

  const hotkeyFieldValue = capturing
    ? "Press a key combination… (Esc to cancel)"
    : (pendingHotkey ?? settings.hotkey);
  const hotkeyPending = !capturing && pendingHotkey !== null && pendingHotkey !== settings.hotkey;

  return (
    <div className="flex max-w-md flex-col gap-6" data-testid="general-panel">
      <div className="flex flex-col gap-1">
        <label htmlFor="hotkey-input" className="text-sm font-medium">
          Hotkey
        </label>
        <div className="flex items-center gap-2">
          <input
            id="hotkey-input"
            data-testid="hotkey-input"
            ref={hotkeyInputRef}
            type="text"
            readOnly
            value={hotkeyFieldValue}
            onFocus={beginCapture}
            onBlur={handleHotkeyBlur}
            onKeyDown={handleHotkeyKeyDown}
            className="flex-1 rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:outline-none dark:border-neutral-700 dark:bg-neutral-950"
          />
          <button
            type="button"
            data-testid="hotkey-apply-button"
            onClick={handleApplyHotkey}
            disabled={!canApplyHotkey}
            className="rounded-md bg-blue-600 px-3 py-2 text-sm font-medium text-white disabled:cursor-not-allowed disabled:opacity-50 hover:bg-blue-500"
          >
            Apply
          </button>
        </div>
        {hotkeyPending && (
          <p data-testid="hotkey-pending" className="text-xs text-amber-600 dark:text-amber-400">
            Pending change — click Apply to save, or Esc/refocus to discard.
          </p>
        )}
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
