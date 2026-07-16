import { useState } from "react";
import { GeneralTab } from "./GeneralTab";
import { HistoryTab } from "./HistoryTab";
import { TABS, type TabId } from "./tabs";

/**
 * Settings window — tabbed settings UI (hotkey, model, output-target
 * configuration; see docs/ARCHITECTURE.md §Project Structure).
 *
 * Talks to the core only through `src/lib/ipc.ts` (via each tab's own
 * component), so this window renders in a plain browser (mocked IPC) for
 * Playwright visual verification.
 *
 * General tab lands in issue #126 (M2 PR 2.5); History lands in issue #199
 * (M3 PR 3.3). The rest (Dictionary/Tone/Snippets) are later M3+
 * increments — clicking one shows a placeholder rather than being
 * disabled, so the tab bar's final shape (and switching between tabs) is
 * exercised now without pulling forward content that doesn't exist yet.
 */
export function SettingsWindow() {
  const [active, setActive] = useState<TabId>("general");
  const activeLabel = TABS.find((t) => t.id === active)?.label ?? active;

  return (
    <main className="flex h-screen w-screen bg-neutral-50 text-neutral-900 dark:bg-neutral-900 dark:text-neutral-100">
      <nav
        role="tablist"
        aria-label="Settings sections"
        className="flex w-40 shrink-0 flex-col gap-0.5 border-r border-neutral-200 p-2 text-sm dark:border-neutral-800"
      >
        {TABS.map((tab) => (
          <button
            key={tab.id}
            type="button"
            role="tab"
            data-testid={`tab-${tab.id}`}
            aria-selected={active === tab.id}
            onClick={() => setActive(tab.id)}
            className={`rounded-md px-3 py-1.5 text-left ${
              active === tab.id
                ? "bg-neutral-200 font-medium text-neutral-900 dark:bg-neutral-800 dark:text-neutral-100"
                : "text-neutral-500 hover:bg-neutral-100 dark:text-neutral-400 dark:hover:bg-neutral-800/50"
            }`}
          >
            {tab.label}
          </button>
        ))}
      </nav>
      <section role="tabpanel" className="flex-1 overflow-y-auto p-6">
        <h1 className="mb-4 text-lg font-semibold">Settings</h1>
        {active === "general" ? (
          <GeneralTab />
        ) : active === "history" ? (
          <HistoryTab />
        ) : (
          <p
            data-testid="placeholder-panel"
            className="text-sm text-neutral-500 dark:text-neutral-400"
          >
            {activeLabel} settings are coming in a later M3 increment.
          </p>
        )}
      </section>
    </main>
  );
}
