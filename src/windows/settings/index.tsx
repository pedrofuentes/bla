/**
 * Settings window — tabbed settings UI (hotkey, model, output-target
 * configuration; see docs/ARCHITECTURE.md §Project Structure).
 *
 * Talks to the core only through `src/lib/ipc.ts`, so this window renders in
 * a plain browser (mocked IPC) for Playwright visual verification.
 *
 * Placeholder shell (issue #126, M2 PR 2.1): the window's title-bar-free
 * shape with a General tab placeholder — no `get_settings`/`set_settings`
 * wiring or real tabs yet; the tray's "Settings…" item already shows +
 * focuses the real OS window around whatever this renders. Full tabbed
 * content is a later M2 increment.
 */
export function SettingsWindow() {
  return (
    <main className="flex h-screen w-screen bg-neutral-50 text-neutral-900 dark:bg-neutral-900 dark:text-neutral-100">
      <nav className="w-40 shrink-0 border-r border-neutral-200 p-4 text-sm dark:border-neutral-800">
        <p className="font-medium text-neutral-500 dark:text-neutral-400">General</p>
      </nav>
      <section className="flex-1 p-6">
        <h1 className="text-lg font-semibold">Settings</h1>
        <p className="mt-2 text-sm text-neutral-500 dark:text-neutral-400">
          Full settings coming in a later M2 increment.
        </p>
      </section>
    </main>
  );
}
