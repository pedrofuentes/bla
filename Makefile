# bla — dev/CI command surface (AGENTS.md §Commands mirrors these).
# Rust commands run with --manifest-path so `make` works from the repo root.

CARGO_MANIFEST := src-tauri/Cargo.toml

.PHONY: check clippy fmt fmt-check test coverage ui-install ui-build ui-test ui-lint ui-format-check ci

## Rust core (src-tauri) ------------------------------------------------------

check: ## cargo check
	cargo check --manifest-path $(CARGO_MANIFEST)

clippy: ## clippy, CI-ready: zero warnings allowed
	cargo clippy --manifest-path $(CARGO_MANIFEST) --all-targets -- -D warnings

fmt: ## apply rustfmt
	cargo fmt --manifest-path $(CARGO_MANIFEST)

fmt-check: ## rustfmt in check mode (CI)
	cargo fmt --manifest-path $(CARGO_MANIFEST) --check

test: ## cargo test
	cargo test --manifest-path $(CARGO_MANIFEST)

# cargo-llvm-cov coverage (MISSION.md §7: 70% threshold on core Rust logic).
#
# Coverage-exclusion policy: `audio`, `output`, `hotkeys`, and `context` are
# the OS-integration modules (AGENTS.md §OS-integration exemption) — thin
# platform glue (device open, synthetic keystrokes, tray/window management,
# permission prompts) that is TDD-exempt and excluded from the coverage floor
# so the ratchet only ever tracks pure, testable logic (`cleanup`, `store`,
# path-templating/tone/snippet code, `stt`'s pre/post-processing). Install
# once with `cargo install cargo-llvm-cov`; requires `rustup component add
# llvm-tools-preview`.
coverage: ## cargo-llvm-cov HTML report, OS-glue modules excluded from the count
	cargo llvm-cov --manifest-path $(CARGO_MANIFEST) --workspace --html \
		--ignore-filename-regex 'src-tauri/src/(audio|output|hotkeys|context)\.rs'

## UI (React + Vite + Tailwind) -----------------------------------------------

ui-install: ## install UI deps
	pnpm install

ui-build: ## typecheck + Vite build
	pnpm build

ui-test: ## Vitest
	pnpm test

ui-lint: ## ESLint
	pnpm lint

ui-format-check: ## Prettier check
	pnpm format:check

## Aggregate -------------------------------------------------------------------

ci: check clippy fmt-check test ui-install ui-build ui-test ui-lint ui-format-check ## full local CI gate
