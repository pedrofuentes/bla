import { defineConfig, configDefaults } from "vitest/config";
import react from "@vitejs/plugin-react";

// UI unit tests (MISSION.md §7: UI components counted separately from the
// core-Rust 70% coverage threshold). No coverage floor is enforced here yet —
// there are no behavior-bearing components in the scaffold; a threshold lands
// with the first real component per the TDD rules in AGENTS.md.
//
// Kept standalone from vite.config.ts (which exports an async factory function
// for Tauri dev-server options) so `mergeConfig` isn't needed for the one
// plugin (`react()`) both configs share.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    // Issue #213: a spy (e.g. `vi.spyOn(console, "error")`) that's only
    // restored on a test's happy path can leak into later tests in the same
    // file if an assertion throws first — silencing their real error output
    // during a red run. `restoreMocks` calls `vi.restoreAllMocks()`
    // automatically before every test, so restoration no longer depends on
    // a test body reaching its own `mockRestore()` call. Verified against
    // the full suite (13 files / 196 tests, all still green) — the other
    // spy/mock users (`GeneralTab.test.tsx`, `settings/index.test.tsx`) only
    // ever set behavior via `beforeEach`, which reruns after this hook.
    restoreMocks: true,
    // Scaffold has no behavior-bearing components yet (AGENTS.md scaffolding
    // exemption); drop this once the first real `*.test.tsx` lands.
    passWithNoTests: true,
    exclude: [...configDefaults.exclude, "**/.worktrees/**", "**/.claude/worktrees/**"],
    coverage: {
      provider: "v8",
      reporter: ["text", "html"],
      // Windows are thin view shells wired to src-tauri via lib/ipc.ts;
      // exclude generated/config files from the coverage report.
      exclude: ["**/*.config.*", "src-tauri/**", "src/main.tsx", "src/vite-env.d.ts"],
    },
  },
});
