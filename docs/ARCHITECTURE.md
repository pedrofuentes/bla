# Architecture

> Extended architectural context for AI agents. Referenced from AGENTS.md.

---

## Project Structure

```
bla/
├── src-tauri/                   ← Rust core (Tauri 2)
│   ├── src/
│   │   ├── main.rs              ← Tauri setup, tray, window management
│   │   ├── hotkeys.rs           ← global shortcut registration, hold/toggle state machine
│   │   ├── audio.rs             ← cpal capture → 16 kHz mono f32 ring buffer
│   │   ├── stt.rs               ← whisper-rs transcription (dictionary terms as initial_prompt)
│   │   ├── cleanup.rs           ← Cleanup trait + RegexCleanup + OllamaCleanup
│   │   ├── output.rs            ← output router: clipboard-swap paste │ templated file append
│   │   ├── context.rs           ← active-app detection → tone profile
│   │   ├── store.rs             ← rusqlite persistence (history, dictionary, snippets)
│   │   └── commands.rs          ← Tauri IPC commands for the UI
│   ├── prompts/                 ← versioned LLM cleanup prompts (rewrite-only)
│   └── tests/                   ← integration tests + WAV fixtures
├── src/                         ← React + TypeScript UI (Vite + Tailwind)
│   ├── windows/settings/        ← tabbed settings window
│   ├── windows/pill/            ← always-on-top recording pill
│   └── lib/ipc.ts               ← typed Tauri IPC wrappers (mockable for browser/Playwright)
├── docs/                        ← this documentation + kickoff docs
├── MISSION.md                   ← binding project brief
├── AGENTS.md                    ← agent instructions (MUST rules)
└── ROADMAP.md                   ← milestones
```

*(Scaffold lands in M1; keep this tree current as modules appear.)*

## Key Technical Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Shell | Tauri 2 over Electron | audio + STT are native; whisper-rs runs in-process in the Rust core instead of a sidecar; ~10 MB app |
| STT | whisper-rs (whisper.cpp), Metal on macOS | fully on-device (MISSION §5); large-v3-turbo quantized ≈ real-time; `small` preset for low RAM |
| Cleanup | `Cleanup` trait: Ollama LLM + regex fallback | privacy (localhost only), and the app stays fully functional without Ollama |
| Text insertion | clipboard-swap + synthesized paste (enigo) | universal across apps; per-char synthetic typing is slow and IME-fragile |
| Transcription timing | on-release, not streaming | paragraph-level dictation UX; streaming Whisper adds complexity for little gain |
| Persistence | rusqlite + tauri-plugin-store | local-only user data (MISSION §5); no server |

Record new decisions as ADRs in `DECISIONS.md`. The binding versions of the pipeline decisions are ADR-0002…ADR-0007 there: module structure & data flow (ADR-0002), the `ClipboardPayload` no-log wrapper and clipboard-restore semantics (ADR-0003), STT model management (ADR-0004), cleanup layering (ADR-0005), the settings/records persistence split (ADR-0006), and synthetic/public-only test fixtures (ADR-0007).

## Module Boundaries

- `cleanup`, `store`, and the path-templating/snippet/tone logic are **pure logic** — no OS calls, fully unit-testable, TDD-mandatory.
- `audio`, `output`, `hotkeys`, `context` are the **only** modules that touch platform APIs (see AGENTS.md §OS-integration exemption); they stay thin and delegate all decisions to pure logic.
- Clipboard text moves through `output` only inside the `ClipboardPayload` wrapper — no `Debug`/`Display`/`Serialize`, compile-time trait-asserted; restore is delayed (150–300 ms, configurable) and skipped if the clipboard changed meanwhile (ADR-0003, PRD AC-9).
- The UI talks to the core **only** through `commands.rs` IPC; `src/lib/ipc.ts` wraps every call so the UI runs in a plain browser with mocks for Playwright screenshots.

## Data Flow

```
hold hotkey ──► audio (cpal ring buffer)
release     ──► stt (whisper-rs, dictionary as initial_prompt)
            ──► cleanup (Ollama w/ tone prompt │ regex fallback)
            ──► output router ── cursor: clipboard-swap paste
            │                 └─ file: append to {{date:…}}-templated .md
            └─► store (history, SQLite)
```

Privacy invariant (MISSION §5): nothing in this flow touches the network except `localhost:11434`; model downloads are a separate, user-initiated path to huggingface.co.

## Key Files

| File | Purpose |
|------|---------|
| `MISSION.md` | binding brief: scope, privacy rules, authorization tiers, acceptance criteria |
| `src-tauri/src/cleanup.rs` | the Cleanup trait — all text-transform behavior hangs off it |
| `src-tauri/src/output.rs` | output router — the only paste/file-write path |
| `src-tauri/prompts/` | versioned LLM prompts with fixture regression checks |
| `src-tauri/tests/fixtures/` | WAV + transcript fixtures backing AC-1/AC-2/AC-4 — synthetic (TTS) or public-domain only, never real recordings (ADR-0007) |
