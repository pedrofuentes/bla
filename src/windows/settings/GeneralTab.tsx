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
import { captureEndNeedsResume, type CaptureEndReason } from "../../lib/hotkeyCapture";
import { applySettingsPatch } from "../../lib/settingsPatch";

const MODEL_PRESETS: readonly ModelPreset[] = ["LargeV3Turbo", "Small"];

type SaveStatus = "idle" | "saving" | "saved";

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
 * `onChange` merges just its own field into the latest known settings
 * snapshot (`applySettingsPatch`, issue #183) and calls `set_settings`
 * immediately, showing a brief "Saved" confirmation (`saveStatus`) or an
 * inline `save-error` on failure. The hotkey field is the one exception:
 * issue #91's validate-before-persist invariant still applies — a captured
 * chord is validated first, and only a chord that validates is auto-applied;
 * an invalid one shows an inline error and is never sent to `set_settings`.
 *
 * Issue #181 (AC-7 smoke test): while the hotkey-capture field is focused,
 * the still-live global dictation shortcut used to keep firing (starting a
 * dictation) instead of the keypress being captured for rebinding. Focusing
 * the field calls `suspend_hotkey` (minting a monotonic generation token);
 * capture ending calls `resume_hotkey` with that same token unless a
 * committed chord actually CHANGED the hotkey — in which case that save's
 * `set_settings` re-registers the new binding itself. `captureEndNeedsResume`
 * (`src/lib/hotkeyCapture.ts`) is the pure decision of which reasons need the
 * explicit resume, including PR #185 Sentinel 🔴-1(a): re-pressing the
 * CURRENT chord commits an *unchanged* hotkey, so `set_settings` re-registers
 * nothing and the field must resume itself. Two more #185 safety nets keep
 * the global hotkey from being left permanently dead: the effect cleanup
 * resumes if the component unmounts mid-capture (🔴-1(b), React side; the
 * hidden-not-destroyed window path is covered backend-side in `lib.rs`), and
 * every `suspend_hotkey`/`resume_hotkey` invoke has a `.catch` that surfaces
 * an OS rejection as a save error rather than swallowing it (🟡-3). The
 * generation token (🔴-1(iii)) lets the backend reject a stale, out-of-order
 * resume so it can't re-enable the shortcut during a newer capture.
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
 * The latest settings are held in a `useRef` (PR #185 Sentinel 🔴-2), not
 * just React state: every auto-apply merges its patch against the ref and
 * updates it *synchronously* before awaiting `set_settings`, so two controls
 * toggled in quick succession each build on the prior change instead of a
 * stale closed-over snapshot — otherwise the later, slower write would
 * silently revert the earlier field (a lost update). The ref is also how the
 * `output-mode-changed` subscription (PR #134 Sentinel 🔴-2, mirroring
 * `App.tsx`) keeps a concurrent tray-/status-window mode switch from being
 * clobbered by the next auto-apply.
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

  // PR #185 Sentinel 🔴-2: the synchronously-updated source of truth every
  // auto-apply builds its patch on (see the component doc comment). Kept in
  // lock-step with the `settings` state.
  const settingsRef = useRef<Settings | null>(null);
  // PR #185 Sentinel 🔴-1(iii): the generation of the current outstanding
  // hotkey-capture suspend (null when not suspended), and the monotonic
  // counter that mints them. `resume_hotkey` echoes the active generation so
  // the backend can reject a stale, out-of-order resume.
  const suspendGenCounterRef = useRef(0);
  const activeSuspendGenRef = useRef<number | null>(null);

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
      for (const unlisten of active) unlisten();
      // PR #185 Sentinel 🔴-1(b): if the component unmounts while a capture
      // suspend is still outstanding, restore the global shortcut so it
      // can't be left dead. (The settings window is hidden — not destroyed —
      // on close, so this rarely fires on a real close; that path is covered
      // backend-side by `force_resume_hotkey` in lib.rs.)
      const pendingGen = activeSuspendGenRef.current;
      if (pendingGen !== null) {
        activeSuspendGenRef.current = null;
        void invoke("resume_hotkey", { generation: pendingGen }).catch(() => {
          /* unmounting — nothing left to surface the error on */
        });
      }
    };
  }, []);

  // Issue #181 / PR #185 🔴-1(iii): unregister the global shortcut and mint
  // a fresh generation as the current outstanding suspend. 🟡-3: surface an
  // OS rejection instead of swallowing it.
  const suspendHotkey = useCallback(() => {
    const generation = ++suspendGenCounterRef.current;
    activeSuspendGenRef.current = generation;
    void invoke("suspend_hotkey", { generation }).catch((err) => setSaveError(String(err)));
  }, []);

  // Issue #181 / PR #185 🔴-1(iii): restore the shortcut, echoing the active
  // suspend's generation so the backend rejects it if a newer suspend has
  // since superseded it. No-op if there's no outstanding suspend. 🟡-3:
  // surface an OS rejection rather than leaving the hotkey silently dead.
  const resumeHotkey = useCallback(() => {
    const generation = activeSuspendGenRef.current;
    activeSuspendGenRef.current = null;
    if (generation === null) return;
    void invoke("resume_hotkey", { generation }).catch((err) => setSaveError(String(err)));
  }, []);

  // Issue #183: the single merge point every control's auto-apply goes
  // through — see the component doc comment. Reads/writes `settingsRef`
  // synchronously (PR #185 🔴-2) so overlapping auto-applies don't lose
  // updates, so it needs no reactive deps.
  const applySettingsChange = useCallback(
    async (patch: Partial<Settings>) => {
      const base = settingsRef.current;
      if (!base) return;
      const next = applySettingsPatch(base, patch);
      // Synchronous: a second auto-apply firing before this one's IPC
      // resolves builds on THIS change, not the pre-change snapshot.
      settingsRef.current = next;
      setSettings(next);

      setSaveStatus("saving");
      setSaveError(null);

      try {
        await invoke("set_settings", { settings: next });
        setSaveStatus("saved");
        // A changed hotkey is re-registered by set_settings itself, so the
        // capture suspend is now resolved — clear it so no later resume
        // (or the unmount safety net) double-registers.
        if (patch.hotkey !== undefined) {
          activeSuspendGenRef.current = null;
        }
        // Issue #91/#110 pattern reuse: after a preset change auto-applies,
        // re-check the newly-selected preset's on-disk status the same way
        // the initial mount does.
        if (patch.model_preset !== undefined) {
          invoke("download_selected_model")
            .then((result) =>
              setModelStatus(result === "already-present" ? "ready" : "downloading"),
            )
            .catch((err) => {
              setModelStatus("error");
              setSaveError(String(err));
            });
        }
      } catch (err) {
        setSaveStatus("idle");
        setSaveError(String(err));
        // PR #185 🔴-1: if a hotkey save failed, set_settings may have left
        // the shortcut unregistered — restore whatever was registered before
        // capture so it can't be left dead.
        if (patch.hotkey !== undefined) {
          resumeHotkey();
        }
      }
    },
    [resumeHotkey],
  );

  // Issue #181: ends hotkey capture, optionally reverting the field's
  // displayed value, and restores the global shortcut when
  // `captureEndNeedsResume` says this end reason needs it.
  const endCapture = useCallback(
    (reason: CaptureEndReason, revertTo?: string) => {
      setCapturing(false);
      if (revertTo !== undefined) setHotkeyInput(revertTo);
      if (captureEndNeedsResume(reason)) {
        resumeHotkey();
      }
    },
    [resumeHotkey],
  );

  const beginCapture = useCallback(() => {
    setCapturing(true);
    setHotkeyError(null);
    suspendHotkey();
  }, [suspendHotkey]);

  const handleHotkeyBlur = useCallback(() => {
    // Only a genuine "lost focus mid-capture" needs a resume — a blur
    // firing right after a chord already committed (capturing is already
    // false by then) must not re-suspend/resume against a save that may
    // still be in flight.
    if (capturing) {
      endCapture("blur", settingsRef.current?.hotkey ?? hotkeyInput);
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
        endCapture("escape", settingsRef.current?.hotkey ?? hotkeyInput);
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
      invoke("validate_hotkey", { accelerator: chord })
        .then(() => {
          setHotkeyError(null);
          const changed = chord !== (settingsRef.current?.hotkey ?? "");
          if (changed) {
            // set_settings re-registers the new binding as part of the save.
            void applySettingsChange({ hotkey: chord });
          }
          // PR #185 🔴-1(a): an unchanged committed chord re-registers
          // nothing via set_settings, so resume the suspend ourselves.
          if (captureEndNeedsResume("committed", changed)) {
            resumeHotkey();
          }
        })
        .catch((err) => {
          setHotkeyError(String(err));
          endCapture("invalid");
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
