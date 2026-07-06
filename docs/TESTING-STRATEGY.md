# Testing Strategy

> Extended testing context for AI agents. Referenced from AGENTS.md.
> **The TDD mandate (tests before implementation) is enforced in AGENTS.md and verified by Sentinel.**
> This document covers the details of HOW to test.

---

## Test Types

| Type | Purpose | Location | Runner |
|------|---------|----------|--------|
| Unit (Rust) | Pure logic: cleanup transforms, snippet matching, path templating, tone rules, clipboard-restore decisions | `#[cfg(test)]` modules | cargo test |
| Unit (UI) | React components, IPC wrappers | `src/**/*.test.tsx` | Vitest |
| Integration | Headless pipeline: WAV fixture → whisper → cleanup → output (temp dir); the cumulative `AC-n` acceptance suite | `src-tauri/tests/` | cargo test |
| Visual | Settings window + pill states rendered in a browser with mocked IPC, screenshotted for the design loop | `tests/visual/` | Playwright vs Vite dev server |

No full-app e2e driver exists on macOS for Tauri — final in-app verification is the human smoke-test gate (MISSION.md AC-7).

## Coverage Requirements

- **New code**: 70% diff coverage required (lines added/modified in the PR) — pure-logic modules; OS-integration glue (`audio`, `output`, `hotkeys`, `context` platform calls) is excluded via coverage config
- **Project-wide coverage**: must never decrease from the previous merge baseline (Sentinel ratchet)
- **Critical paths**: 100% coverage required (cleanup fallback path, output routing, path templating — anything between "speech recognized" and "text delivered")
- **Run coverage**: `cargo llvm-cov` (Rust) / `pnpm test -- --coverage` (UI)
- **Sentinel verifies coverage thresholds on every PR**

## Test-Only PRs

PRs that only add tests to existing (untested) code use commit type `test(scope)` and are exempt from test-first choreography ordering (there is no `feat`/`fix` to follow). Sentinel verifies the tests are meaningful and pass.

## Testing Patterns

### Mocking
Dependency injection via traits — the same seam the architecture already uses. `Cleanup` is a trait, so pipeline tests inject a `FakeCleanup`; the Ollama client takes a base URL, so fallback tests point it at an unbound port; output targets take a root path, so file-mode tests write into `tempfile::tempdir()`. Whisper inference is exercised only in integration tests against small fixture WAVs (or a `FakeStt` returning canned transcripts for pipeline-shape tests). Never mock what you own and can run locally.

```rust
// Example: testing the pipeline's cleanup fallback without a network
struct UnreachableOllama;
impl Cleanup for UnreachableOllama {
    fn clean(&self, _raw: &str, _tone: Tone) -> Result<String, CleanupError> {
        Err(CleanupError::Unreachable)
    }
}

#[test]
fn falls_back_to_rules_when_llm_unreachable() {
    let pipeline = Pipeline::new(UnreachableOllama, RegexCleanup::default());
    let out = pipeline.clean("um so let's meet Tuesday", Tone::Neutral).unwrap();
    assert_eq!(out, "So let's meet Tuesday.");
}
```

### Test Naming Convention
```rust
// Rust: #[test] fn <behavior>_when_<condition>()
#[test]
fn resolves_self_correction_when_speaker_amends_day() { /* Arrange → Act → Assert */ }
```
```typescript
// UI (Vitest)
describe('SettingsGeneral', () => {
  it('should persist hotkey change when the recorder captures a chord', () => {
    // Arrange → Act → Assert
  });
});
```

### What Must Be Tested
- All public API functions
- Error paths and edge cases (Ollama down, model file missing, clipboard changed mid-restore, template path for a date that has no file yet)
- State transitions (hold/toggle hotkey state machine; pill states)
- Input validation and boundary conditions (empty audio, sub-300 ms accidental press, 100+-second utterance)
- Privacy guard: AC-5 asserts no runtime connection outside the MISSION.md §5 allowlist

### What Should NOT Be Tested
- Framework internals (Tauri window/tray plumbing)
- Third-party library behavior (whisper.cpp decoding quality per se — assert on fixture outcomes, not on model internals)
- Implementation details (test behavior, not structure)

## CI Integration

- Tests run automatically on every PR via GitHub Actions (macOS runner; Windows compile-check job)
- All tests must pass before Sentinel review begins
- Flaky tests must be fixed immediately, not skipped
