import { useCallback, useEffect, useState } from "react";
import { invoke, onEvent, type Settings } from "./lib/ipc";
import {
  hotkeyInstruction,
  modeLabel,
  modelPresetLabel,
  modelStatusLabel,
  otherMode,
  statusLabel,
  type ModelStatus,
  type PipelineState,
} from "./lib/status";

/** Tailwind classes for the small colored dot next to the status line. */
const STATUS_DOT_CLASSES: Record<PipelineState, string> = {
  Idle: "bg-neutral-400 dark:bg-neutral-500",
  Active: "bg-red-500",
  Busy: "bg-blue-500",
  Error: "bg-red-600",
  Unknown: "bg-neutral-300 dark:bg-neutral-600",
};

/**
 * Minimal status window (issue #110, MISSION §2 "invisible until
 * summoned"): reflects the live hotkey, output mode, and selected model —
 * everything renders straight from `Settings` plus the events the core
 * already emits (`src-tauri/src/lib.rs`). No decision logic lives here;
 * every formatting/labeling choice is a pure helper from `src/lib/status.ts`
 * so it's unit-tested without a live Tauri app context. The full tabbed
 * settings UI is M2 — this window only shows a read-only summary.
 */
function App() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [pipelineState, setPipelineState] = useState<PipelineState>("Unknown");
  const [modelStatus, setModelStatus] = useState<ModelStatus>("checking");
  const [downloadPercent, setDownloadPercent] = useState<number | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    invoke("get_settings")
      .then((loaded) => {
        if (!cancelled) setSettings(loaded);
      })
      .catch((err) => {
        if (!cancelled) setError(String(err));
      });

    invoke("download_selected_model")
      .then((result) => {
        if (cancelled) return;
        setModelStatus(result === "already-present" ? "ready" : "downloading");
      })
      .catch((err) => {
        if (!cancelled) {
          setModelStatus("error");
          setError(String(err));
        }
      });

    const unlisten = Promise.all([
      onEvent("pipeline-state-changed", (state) => {
        if (!cancelled) setPipelineState(state as PipelineState);
      }),
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
          setError(message);
        }
      }),
      onEvent("output-mode-changed", (mode) => {
        if (!cancelled) {
          setSettings((prev) => (prev ? { ...prev, output_mode: mode } : prev));
        }
      }),
    ]);

    return () => {
      cancelled = true;
      unlisten.then((fns) => fns.forEach((fn) => fn()));
    };
  }, []);

  const toggleOutputMode = useCallback(async () => {
    if (!settings) return;
    const nextMode = otherMode(settings.output_mode);
    try {
      await invoke("set_output_mode", { mode: nextMode });
      setSettings({ ...settings, output_mode: nextMode });
    } catch (err) {
      setError(String(err));
    }
  }, [settings]);

  return (
    <main className="flex min-h-screen flex-col gap-6 bg-neutral-50 p-6 font-sans text-neutral-900 dark:bg-neutral-900 dark:text-neutral-100">
      <header className="flex items-center gap-3">
        <span
          aria-hidden
          className={`h-2.5 w-2.5 shrink-0 rounded-full ${STATUS_DOT_CLASSES[pipelineState]}`}
        />
        <div>
          <p className="text-xs font-medium tracking-wide text-neutral-500 uppercase dark:text-neutral-400">
            bla
          </p>
          <p className="text-base font-medium">{statusLabel(pipelineState)}</p>
        </div>
      </header>

      {error && (
        <p className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700 dark:border-red-900 dark:bg-red-950 dark:text-red-300">
          {error}
        </p>
      )}

      {!settings ? (
        <p className="text-sm text-neutral-500 dark:text-neutral-400">Loading…</p>
      ) : (
        <>
          <section className="rounded-lg border border-neutral-200 bg-white p-4 dark:border-neutral-800 dark:bg-neutral-950">
            <p className="text-lg">{hotkeyInstruction(settings.recording_mode, settings.hotkey)}</p>
          </section>

          <section className="flex flex-col gap-3 text-sm">
            <div className="flex items-center justify-between rounded-md border border-neutral-200 px-3 py-2 dark:border-neutral-800">
              <span>
                Output: <strong>{modeLabel(settings.output_mode)}</strong>
              </span>
              <button
                type="button"
                onClick={toggleOutputMode}
                className="rounded-md border border-neutral-300 px-2.5 py-1 text-xs font-medium hover:border-neutral-400 dark:border-neutral-700 dark:hover:border-neutral-500"
              >
                Switch to {modeLabel(otherMode(settings.output_mode))}
              </button>
            </div>

            <div className="flex items-center justify-between rounded-md border border-neutral-200 px-3 py-2 dark:border-neutral-800">
              <span>{modelPresetLabel(settings.model_preset)}</span>
              <span className="text-neutral-500 dark:text-neutral-400">
                {modelStatusLabel(modelStatus, downloadPercent)}
              </span>
            </div>
          </section>

          <section className="border-t border-neutral-200 pt-4 text-xs text-neutral-500 dark:border-neutral-800 dark:text-neutral-400">
            <p className="mb-1 font-medium text-neutral-600 dark:text-neutral-300">Settings</p>
            <p>
              Hotkey: {settings.hotkey} · Mode: {settings.recording_mode} · Output:{" "}
              {modeLabel(settings.output_mode)} · Model: {modelPresetLabel(settings.model_preset)}
            </p>
            <p className="mt-2 italic">Full settings coming in M2.</p>
          </section>
        </>
      )}
    </main>
  );
}

export default App;
