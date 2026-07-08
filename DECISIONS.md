# Architecture Decision Records — bla

> **Record every significant technical decision here.** When choosing between approaches,
> document what was chosen and why. This prevents future agents and developers from
> re-debating settled decisions or accidentally reversing them.
>
> Do NOT write decisions to AGENTS.md — they belong here.

## Format

```markdown
### ADR-NNN: Decision Title
**Date**: YYYY-MM-DD
**Status**: Proposed / Accepted / Superseded by ADR-NNN
**Context**: What problem or question prompted this decision?
**Decision**: What was decided?
**Alternatives considered**: What other options were evaluated?
**Consequences**: What are the trade-offs? What does this enable or prevent?
```

## Decisions

<!-- Add new decisions below this line, most recent first -->

### ADR-0007: Test fixtures are synthetic or already-public only — never real recordings
**Date**: 2026-07-06
**Status**: Accepted
**Context**: PRD AC-1/AC-2 require speech-fixture WAVs committed to the repo, and MISSION §7 requires fixture-based regression checks for cleanup prompts. But MISSION §5's "audio, transcripts, and derived text never leave the device" invariant is scoped to product runtime, while the build fleet's engines transmit repo content to their backends by design — so any real recording committed as a fixture would leave the device (Sentinel issue #9).
**Decision**: Every audio or transcript fixture in the repo must be synthetic or already-public: generated via TTS (with fillers/self-corrections scripted in), or sourced from public-domain audio/text. Real user recordings — including the cofounder's own voice — are never committed. Fixture provenance is noted alongside the fixture (a one-line source note in `src-tauri/tests/fixtures/`).
**Alternatives considered**: Git-ignored local-only fixtures (breaks CI reproducibility of AC-1/AC-2); recording the cofounder's voice with consent (still transmits personal voiceprint data through fleet engines, contradicting the spirit of MISSION §5).
**Consequences**: Repo content is safe to transmit through any build-fleet engine; fixtures are reproducible from scripts. Trade-off: synthetic speech is less acoustically diverse than real dictation — acceptable because the recurring AC-7 human smoke test covers real-voice behavior at every milestone.

### ADR-0006: Persistence split — tauri-plugin-store for settings, rusqlite for user data
**Date**: 2026-07-06
**Status**: Accepted
**Context**: MISSION §3 names both `tauri-plugin-store` and `rusqlite`; MISSION §5 requires history/dictionary to live in local SQLite under the OS app-data dir. PRD AC-13 needs settings that round-trip across restarts; AC-20/AC-21/AC-24 (M3/M4) need searchable, deletable, retention-managed records. One store cannot serve both well.
**Decision**: Split by data shape. **Settings** (hotkey binding, hold/toggle mode, selected model preset, output mode, file-path template — the AC-13 set) live in `tauri-plugin-store` (JSON, IPC-readable by the UI). **User-generated records** — history (raw + cleaned), personal dictionary, snippets — live in rusqlite under the OS app-data dir (AC-20, MISSION §5). The `store` module owns the SQLite schema and the retention policy: purging entries older than the configured age (AC-20) is `store`'s responsibility, not the caller's. Neither store's files are ever committed to the repo (MISSION §7 NEVER list).
**Alternatives considered**: SQLite for everything (loses the plugin's free UI-side settings sync and versioned-key handling for trivial key-value data); store-plugin for everything (no substring search, per-row delete, or retention over growing history data).
**Consequences**: Settings stay a flat, mockable key-value surface for the UI; record data gets real queries. Trade-off: two persistence mechanisms to test — accepted because both are pure-logic-testable (`store` is a TDD-mandatory pure module — pure logic over an injected DB path — per ADR-0002) and AC-13/AC-20 already prescribe separate test harnesses.

### ADR-0005: Cleanup layering — `Cleanup` trait, always-on regex baseline, optional Ollama pass
**Date**: 2026-07-06
**Status**: Accepted
**Context**: MISSION §4 requires a pluggable cleanup layer: rule-based always available, LLM cleanup when a local Ollama responds, graceful fallback otherwise (PRD AC-4, AC-10). MISSION §7 mandates that cleanup stays behind the `Cleanup` trait and that LLM prompts are rewrite-only, versioned, and regression-checked. M3 tone profiles (PRD AC-22) will dispatch across implementations.
**Decision**: All text transformation hangs off a `Cleanup` trait in `cleanup.rs`. **`RegexCleanup`** is the always-on baseline: deterministic filler-word removal, spacing, and capitalization — no I/O, fully unit-testable. **`OllamaCleanup`** is an optional layer over `localhost:11434` (the only permitted runtime origin besides model download, MISSION §5): self-corrections, punctuation, spoken lists → bullets. If Ollama is unreachable or errors, the pipeline falls back to the regex result with no error surfaced to the paste path (AC-4). LLM prompts are rewrite-only (never answer, never add content), live in versioned files under `src-tauri/prompts/`, and every prompt change must pass fixture-based regression tests asserting no content beyond the input is introduced (MISSION §7, AC-10).
**Alternatives considered**: LLM-only cleanup (app breaks without Ollama, violating MISSION §4's graceful-fallback requirement); a single configurable pipeline function instead of a trait (blocks M3 per-app tone dispatch and the AC-22 verbatim bypass).
**Consequences**: The app is fully functional with zero LLM dependency; tone profiles and the verbatim bypass slot in as trait dispatch in M3. Because `RegexCleanup` does not resolve self-corrections and CI has no live Ollama, AC-1 runs the pipeline against the stubbed LLM endpoint that AC-10 defines, so the LLM path (and AC-1's corrected-phrase assertion) is exercised deterministically in `cargo test`. Trade-off: two cleanup paths to keep consistent — mitigated by shared fixture transcripts exercised against both (AC-10 pins the LLM path against the same stub).

### ADR-0004: STT engine and model management — whisper-rs/Metal, app-data model dir, allowlisted download
**Date**: 2026-07-06
**Status**: Accepted
**Context**: MISSION §3 fixes `whisper-rs` (Metal on macOS) as the STT engine; MISSION §5 forbids committing model files and allowlists only `huggingface.co` + its CDN for downloads; PRD §4 (Resource footprint) and AC-17 define the model presets; AC-12 constrains the downloader; AC-21 (M3) needs a dictionary-injection seam.
**Decision**: `stt.rs` wraps `whisper-rs` (whisper.cpp) with the Metal backend on macOS. Two supported presets (AC-17): quantized **`large-v3-turbo` (q5)** as the default (≈ real-time on Apple Silicon within the AC-2 latency budget) and **`small`** as the fast/low-RAM option. Models are stored under the OS app-data directory — never in the repo (MISSION §7 NEVER list) — and fetched by a first-run, user-initiated downloader that may contact only `huggingface.co` and its CDN, enforced by a network-guard test scoped to the downloader path (AC-12, MISSION §5). Whisper's `initial_prompt` is the designated dictionary seam: M1 passes it empty; M3 injects personal-dictionary terms there (and into the cleanup prompt) per AC-21, with no `stt.rs` API change.
**Alternatives considered**: A whisper.cpp sidecar process (Tauri-2-over-Electron rationale in `docs/ARCHITECTURE.md` already chose in-process for latency and footprint); bundling a model in the release artifact (bloats the .dmg by gigabytes and hard-codes one preset; the first-run download keeps artifacts small and model choice user-controlled).
**Consequences**: Fully on-device STT with a one-time, auditable download path; preset selection is a settings value (ADR-0006) the M2 model picker reuses. Trade-off: first run requires one network fetch before dictation works — accepted by MISSION §5's explicit allowlist and surfaced by the first-run downloader UX (AC-12).

### ADR-0003: Clipboard no-log wrapper — `ClipboardPayload` with compile-time trait assertions
**Date**: 2026-07-06
**Status**: Accepted
**Context**: MISSION §5/§7 require that raw clipboard contents are never logged or persisted, but prescribe no mechanism. PRD AC-9 pre-committed a specific Rust mechanism (a wrapper type lacking `Debug`/`Display`/`Serialize`, enforced by a compile-time trait-assertion test) that no binding document had decided — flagged by Sentinel issue #4 as a provenance gap. **This ADR supplies the provenance AC-9 lacked**: it adopts AC-9's mechanism as the recorded architectural decision; AC-9's criterion text stands otherwise verbatim, with a one-line citation to this ADR appended in `PRD.md`.
**Decision**: Clipboard text (the outgoing transcript and the saved pre-dictation contents) is carried only inside a **`ClipboardPayload`** wrapper type in the clipboard/output module that implements **neither `Debug`, `Display`, nor `Serialize`**, so it cannot flow into log macros, string formatting, or any serializer by construction. A compile-time trait-assertion test (e.g. `static_assertions::assert_not_impl_any!`) locks this in; removing it or adding one of the traits fails `cargo test`. The payload exposes a single explicit consumption path used only by the paste/restore code. **Restore semantics**: after the synthetic paste, the pre-dictation clipboard is restored following a configurable delay of **150–300 ms** (paste consumers read the clipboard asynchronously; restoring too early truncates the paste). If the clipboard's contents changed after our write — another actor wrote to it — the restore is **skipped**, so `bla` never clobbers newer user data. AC-9's equality assertion ("clipboard equals its pre-dictation value post-paste") applies to the normal, unchanged-clipboard path.
**Alternatives considered**: Restating AC-9's second clause as a mechanism-neutral outcome plus a runtime/log-scan guard (issue #4's first option) — rejected: grep/log-scan guards are advisory and bypassable, while the type-system guard makes the violation unrepresentable; a custom `Debug` impl printing `"<redacted>"` — rejected: still allows accidental `Serialize`/`Display` leaks and weakens the compile-time assertion.
**Consequences**: The no-log invariant is enforced by the compiler, not reviewer vigilance; Sentinel issue #4 is resolved by this ADR plus the one-line AC-9 citation. Trade-offs: slightly less ergonomic debugging around the paste path (intentional), and the restore-delay default must be tuned during the AC-7 smoke test; skip-on-change means AC-9's restore test must also cover the skip branch.

### ADR-0002: Pipeline module structure and data flow
**Date**: 2026-07-06
**Status**: Accepted
**Context**: M1 implements the dictation pipeline (MISSION §4, ROADMAP M1). MISSION §7 mandates no direct OS calls outside designated modules, and AGENTS.md's OS-integration exemption makes thin platform glue TDD-exempt — so the module layout must isolate everything testable from everything platform-bound. `docs/ARCHITECTURE.md` §Project Structure sketches the tree; this ADR makes it binding.
**Decision**: The Rust core is structured as single-responsibility modules along the pipeline's data flow: **hotkey event** (`hotkeys.rs`, hold/toggle state machine — AC-8) **→ audio ring buffer** (`audio.rs`, cpal capture to 16 kHz mono f32) **→ stt** (`stt.rs`, whisper-rs) **→ cleanup** (`cleanup.rs`, `Cleanup` trait — ADR-0005) **→ output router** (`output.rs`, clipboard-swap paste or templated file append — AC-3/AC-9) **→ history** (`store.rs` — ADR-0006). Module boundary rule: `audio`, `output`, `hotkeys`, and `context` are the **only** modules that touch platform APIs; they stay thin and delegate every decision to pure logic (mirroring AGENTS.md's OS-integration exemption). `cleanup`, `store` (pure logic over an injected DB path), path-templating, snippet, and tone logic are **pure** — no OS calls, fully unit-testable, TDD-mandatory. The UI reaches the core only through `commands.rs` IPC. `docs/ARCHITECTURE.md` §Project Structure remains the authoritative tree.
**Alternatives considered**: A monolithic pipeline module (untestable without a live mic/clipboard, breaking AC-1's headless requirement); trait-abstracting every OS call for full mocking (heavier than needed — the thin-glue + pure-core split achieves AC-1/AC-2/AC-8 headless coverage without mock plumbing for platform APIs).
**Consequences**: The acceptance suite runs the whole pipeline headlessly from WAV fixtures (AC-1, AC-2) because every decision point lives in pure code; coverage thresholds apply to the pure core while glue is excluded via coverage config (MISSION §7). Trade-off: some duplication at glue/logic seams (event structs crossing the boundary) — accepted for testability. Later milestones extend the flow without restructuring: tone dispatch (M3) sits at the `cleanup` seam, command mode (M4) reuses `output`'s clipboard path.

---

## ADR-0001 — Engine-per-task fleet policy (Copilot CLI for non-fable work)

- **Date:** 2026-07-06 · **Status:** accepted — attested by the cofounder's merge of PR #6 (merge commit `f0da1f9`, https://github.com/pedrofuentes/bla/pull/6)
- **Decision:** Fleet work is routed per MISSION.md §7: Claude `fable` for architecture, Sentinel review, and native-integration work; headless Copilot CLI for implementation and mechanical work, with Claude `sonnet`/`haiku` fallbacks.
- **Containment:** per Sentinel SNTL-20260706-bla-PR6-23f9e9d, Copilot implementer spawns are blocked until Sentinel-in-CI + harness-guard are required checks on `main`, **or the worker runs under a credential that structurally cannot push or merge to `main`** (read-only Copilot work permitted immediately). Canonical policy text and precondition live in MISSION.md §7.
