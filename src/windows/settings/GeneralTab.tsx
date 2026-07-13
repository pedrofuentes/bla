import { useCallback, useEffect, useState } from "react";
import {
  invoke,
  onEvent,
  type ModelPreset,
  type RecordingMode,
  type Settings,
} from "../../lib/ipc";
import { modelPresetLabel, modelStatusLabel, type ModelStatus } from "../../lib/status";
import { chordFromKeyboardEvent } from "../../lib/hotkeyChord";

const MODEL_PRESETS: readonly ModelPreset[] = ["LargeV3Turbo", "Small"];

type SaveStatus = "idle" | "saving" | "saved";

/**
 * General settings tab (issue #126, M2 PR 2.5): hotkey capture, Whisper
 * model preset (with download progress reusing the pattern from `App.tsx`),
 * and hold-vs-toggle recording mode. Talks to the core only through
 * `src/lib/ipc.ts`.
 *
 * M2 PR 2.6 adds two plain persisted-preference checkboxes: "Launch bla at
 * login" (`launch_at_login` — the backend flips OS autostart registration
 * as a `set_settings` side-effect; see `commands::set_settings`) and "Play
 * sound cues" (`sound_cues` — a pure preference in this PR; cue playback
 * itself is PR 2.7). Both follow the same load-into-local-state /
 * spread-into-`next`-on-save pattern as every other control here.
 *
 * Hotkey save ordering mirrors the backend's validate-before-persist
 * invariant (issue #91 Sentinel 🔴, `settings::persist_validated`): a
 * captured chord is validated via the new `validate_hotkey` command
 * immediately, and `handleSave` refuses to call `set_settings` at all while
 * a validation error is outstanding — an invalid hotkey can never reach
 * `set_settings` from this form.
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
 * The `settings` snapshot tracks `output-mode-changed` (PR #134 Sentinel
 * 🔴-2, mirroring `App.tsx`): the tray menu / status window can flip the
 * output mode while this window is open, and Save spreads the snapshot into
 * a full `set_settings` payload — without the subscription, Save would
 * silently revert + re-persist the concurrent change.
 */
export function GeneralTab() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [hotkeyInput, setHotkeyInput] = useState("");
  const [hotkeyError, setHotkeyError] = useState<string | null>(null);
  const [capturing, setCapturing] = useState(false);
  const [recordingMode, setRecordingMode] = useState<RecordingMode>("Hold");
  const [modelPreset, setModelPreset] = useState<ModelPreset>("LargeV3Turbo");
  const [launchAtLogin, setLaunchAtLogin] = useState(false);
  const [soundCues, setSoundCues] = useState(true);
  const [modelStatus, setModelStatus] = useState<ModelStatus>("checking");
  const [downloadPercent, setDownloadPercent] = useState<number | undefined>(undefined);
  const [saveStatus, setSaveStatus] = useState<SaveStatus>("idle");
  const [saveError, setSaveError] = useState<string | null>(null);
  const [eventsError, setEventsError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    invoke("get_settings")
      .then((loaded) => {
        if (cancelled) return;
        setSettings(loaded);
        setHotkeyInput(loaded.hotkey);
        setRecordingMode(loaded.recording_mode);
        setModelPreset(loaded.model_preset);
        setLaunchAtLogin(loaded.launch_at_login);
        setSoundCues(loaded.sound_cues);
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
      // PR #134 Sentinel 🔴-2 (mirrors App.tsx): keep the snapshot Save
      // spreads in sync with tray-/status-window-triggered mode switches.
      onEvent("output-mode-changed", (mode) => {
        if (!cancelled) {
          setSettings((prev) => (prev ? { ...prev, output_mode: mode } : prev));
        }
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
    };
  }, []);

  const beginCapture = useCallback(() => {
    setCapturing(true);
    setHotkeyError(null);
  }, []);

  const handleHotkeyKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (!capturing) return;
      // Swallow the keydown's default action (typing/Tab/etc.) only while
      // actively capturing a chord — a read-only field with no capture in
      // progress should still let Tab move focus normally.
      e.preventDefault();

      if (e.key === "Escape") {
        setCapturing(false);
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
        .then(() => setHotkeyError(null))
        .catch((err) => setHotkeyError(String(err)));
    },
    [capturing],
  );

  const handleSave = useCallback(async () => {
    if (!settings || hotkeyError) return;

    setSaveStatus("saving");
    setSaveError(null);

    const next: Settings = {
      ...settings,
      hotkey: hotkeyInput,
      recording_mode: recordingMode,
      model_preset: modelPreset,
      launch_at_login: launchAtLogin,
      sound_cues: soundCues,
    };

    try {
      await invoke("set_settings", { settings: next });
      setSettings(next);
      setSaveStatus("saved");
      // Issue #91/#110 pattern reuse: after a successful save, re-check the
      // (possibly just-changed) model preset's on-disk status the same way
      // the initial mount does.
      invoke("download_selected_model")
        .then((result) => setModelStatus(result === "already-present" ? "ready" : "downloading"))
        .catch((err) => {
          setModelStatus("error");
          setSaveError(String(err));
        });
    } catch (err) {
      setSaveStatus("idle");
      setSaveError(String(err));
    }
  }, [settings, hotkeyInput, hotkeyError, recordingMode, modelPreset, launchAtLogin, soundCues]);

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
          onBlur={() => setCapturing(false)}
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
              checked={recordingMode === "Hold"}
              onChange={() => setRecordingMode("Hold")}
            />
            Hold to record
          </label>
          <label className="flex items-center gap-2">
            <input
              type="radio"
              name="recording-mode"
              data-testid="mode-toggle"
              checked={recordingMode === "Toggle"}
              onChange={() => setRecordingMode("Toggle")}
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
          value={modelPreset}
          onChange={(e) => setModelPreset(e.target.value as ModelPreset)}
          className="rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm dark:border-neutral-700 dark:bg-neutral-950"
        >
          {MODEL_PRESETS.map((preset) => (
            <option key={preset} value={preset}>
              {modelPresetLabel(preset)}
            </option>
          ))}
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
            checked={launchAtLogin}
            onChange={(e) => setLaunchAtLogin(e.target.checked)}
          />
          Launch bla at login
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            data-testid="sound-cues-checkbox"
            checked={soundCues}
            onChange={(e) => setSoundCues(e.target.checked)}
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

      <div className="flex items-center gap-3">
        <button
          type="button"
          data-testid="save-button"
          onClick={handleSave}
          disabled={!!hotkeyError || saveStatus === "saving"}
          className="rounded-md bg-blue-600 px-3 py-1.5 text-sm font-medium text-white disabled:cursor-not-allowed disabled:opacity-50 hover:bg-blue-500"
        >
          {saveStatus === "saving" ? "Saving…" : "Save"}
        </button>
        {saveStatus === "saved" && (
          <span
            data-testid="save-status"
            className="text-xs text-neutral-500 dark:text-neutral-400"
          >
            Saved
          </span>
        )}
      </div>
    </div>
  );
}
