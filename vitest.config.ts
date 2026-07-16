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
