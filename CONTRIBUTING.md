# Contributing to bla

Thanks for your interest in contributing. bla is a local-first, on-device voice dictation app (Tauri 2, Rust + React/TypeScript), and it follows a fairly strict, test-first workflow to keep the project reliable. This document summarizes what you need to know as an outside contributor. The full internal workflow lives in [`AGENTS.md`](./AGENTS.md); this is the human-friendly summary.

## Before you start

- Check [ROADMAP.md](./ROADMAP.md) and open issues to see what's planned and avoid duplicate work.
- For anything nontrivial (new dependency, architecture change, new network call, public API change), please open an issue or discussion first — some of these are gated decisions for the project maintainer and may not be accepted even with a working implementation.
- **Never** name, reference, or link a competing dictation product or company anywhere in the repo (code, comments, docs, commit messages, issues, PRs). Describe functionality generically.

## Workflow

1. **Work in a branch/worktree, never on `main`.**
   ```bash
   git fetch origin main
   git worktree add .worktrees/your-change -b feature/your-change main
   cd .worktrees/your-change
   ```
2. **Test-driven development is required.** Write a failing test first, commit it, then write the minimal implementation to make it pass:
   - `test(scope): add failing test` — tests only, must fail
   - `feat|fix(scope): implement` — minimal implementation, suite must pass
   - `refactor(scope): ...` — optional cleanup, suite stays green

   Thin OS-integration glue (audio device open, synthetic keystrokes, tray/window management, permission prompts) is exempt from strict TDD ordering, but keep it minimal and keep real logic out of it so the logic stays testable. Docs, chore, build, and CI changes are also exempt from TDD ordering.
3. **Use conventional commit types**: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `ci`, `style`, `perf`. Format:
   ```
   type(scope): short description
   ```
4. **Before opening a PR**, make sure everything passes locally:
   ```bash
   cargo test
   cargo clippy -- -D warnings
   cargo fmt --check
   pnpm test
   pnpm lint
   ```
   All of the above must be green with zero warnings.
5. **Fixtures must be synthetic or already-public audio/text only.** Never commit real recordings — including your own voice — as test fixtures (see ADR-0007 in [`DECISIONS.md`](./DECISIONS.md)). Generate speech fixtures via TTS or use public-domain audio/text, and note the fixture's source alongside it.
6. **Open a pull request** against `main`. Describe what changed and why, and link any related issue.

## Review process

Every pull request — including small fixes and docs-only changes — goes through an independent review gate ("Sentinel") before it can merge, in addition to human review. As an outside contributor, your PR will be reviewed by a project maintainer; you do not need to run the internal review tooling yourself, but your PR must still pass the automated checks above (tests, clippy, fmt, lint) before it will be merged. Nobody merges their own unreviewed code, and third-party PRs are always reviewed by a human maintainer before merge — automated review alone is not sufficient for external contributions.

## Privacy and security constraints

bla has a hard on-device privacy guarantee (see [MISSION.md](./MISSION.md) for the full brief). Contributions must not:

- Send audio, transcripts, or derived text off the user's device.
- Add any network call from the product outside the existing allowlist (Hugging Face for model downloads, local Ollama on `localhost:11434`).
- Add telemetry, analytics, or crash reporting.
- Log or persist raw clipboard contents.
- Commit model files or user data (history database, settings) to the repo.

If your change needs a new network destination or a new heavy native dependency, flag it explicitly in your PR description — these require maintainer sign-off.

## Building on Windows

bla supports Windows 10/11 as a dev/runtime target alongside macOS. Before building, install:

- **LLVM/libclang** — `winget install LLVM.LLVM`, then set `LIBCLANG_PATH` (e.g. `C:\Program Files\LLVM\bin`). Required because `whisper-rs-sys` generates bindings via `bindgen`, which needs `libclang`. This is the most common first-build failure on a fresh machine.
- **CMake** — `winget install Kitware.CMake` — plus the Visual Studio Build Tools "Desktop development with C++" workload, both needed to compile `whisper.cpp`.
- **WebView2** (present by default on Windows 11 and most updated Windows 10 installs; if missing, install the Evergreen WebView2 Runtime), the **Rust MSVC toolchain** (`rustup default stable-msvc`), and **Node 20+**/**pnpm**.

Then `pnpm install` and `pnpm tauri:dev` (builds `--features whisper`), same as macOS. See the [README](./README.md#building-on-windows) for the full command list.

## Getting help

Environment setup details (Rust, Node/pnpm, Xcode CLT, Ollama, first-run model download, macOS permissions, Windows build prerequisites) are in the [README](./README.md#requirements) and [`docs/DEVELOPMENT-WORKFLOW.md`](./docs/DEVELOPMENT-WORKFLOW.md). If you get stuck, open an issue describing what you're trying to do and what's blocking you.
