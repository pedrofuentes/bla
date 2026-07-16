#!/usr/bin/env python3
"""Screenshot capture for the recording pill's per-listener degrade fix (issue #182).

Drives `tests/visual/pill-harness.html` (Tauri IPC hand-mocked, see that
file's doc comment for why it doesn't use the settings harness's
`shouldMockEvents: true`) with Playwright's Python bindings against a
locally-running Vite dev server, per MISSION.md §3's visual-verification
method. Not a project dependency -- Playwright is a pre-provisioned system
tool on the dev machine (see `tests/visual/capture-screenshots.py`'s
precedent for issue #180); nothing here is imported by the shipped app or
the Vitest/cargo suites.

The pill's chrome (`PillShell` in `src/windows/pill/index.tsx`) is a fixed
dark bubble with no `dark:` variants, so unlike the settings harness this
captures one appearance, not a light/dark pair.

Usage:
    python3 tests/visual/capture-pill-screenshots.py [--base-url http://localhost:4173]

Writes PNGs into tests/visual/screenshots/ (kept out of the production
bundle -- no Vite entry references that directory); the ones worth keeping
for PR review are copied under docs/design/screenshots/ separately.
"""

import argparse
import pathlib

from playwright.sync_api import sync_playwright

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
OUT_DIR = SCRIPT_DIR / "screenshots"

# Matches tauri.conf.json's pill window size (280x80) plus headroom so the
# neutral backdrop reads clearly as "around" the floating pill rather than
# cropping it.
VIEWPORT = {"width": 420, "height": 180}


def capture(base_url: str) -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    with sync_playwright() as p:
        browser = p.chromium.launch()
        context = browser.new_context(viewport=VIEWPORT)
        page = context.new_page()

        # ---- (1) normal: every listener subscribed, live waveform ----
        page.goto(f"{base_url}/tests/visual/pill-harness.html")
        page.wait_for_function("() => typeof window.__pillHarnessEmit === 'function'")
        page.evaluate(
            "window.__pillHarnessEmit('pipeline-state-changed', 'Active')"
        )
        # Feed a few levels so the waveform isn't just its zero-padded rest state.
        for level in (0.01, 0.05, 0.08, 0.03, 0.06, 0.09, 0.02):
            page.evaluate(
                "(l) => window.__pillHarnessEmit('audio-level', l)", level
            )
        page.wait_for_selector('[data-testid="pill-waveform"]')
        page.screenshot(path=str(OUT_DIR / "pill-normal-recording.png"))

        # ---- (2) one listener failed (audio-level): state dot stays alive ----
        page.goto(f"{base_url}/tests/visual/pill-harness.html?fail=audio-level")
        page.wait_for_function("() => typeof window.__pillHarnessEmit === 'function'")
        page.evaluate(
            "window.__pillHarnessEmit('pipeline-state-changed', 'Active')"
        )
        page.wait_for_selector('[data-testid="pill-status-dot"]')
        page.screenshot(path=str(OUT_DIR / "pill-degraded-audio-level-failed.png"))

        # ---- (3) every listener failed: Status unavailable fallback ----
        page.goto(
            f"{base_url}/tests/visual/pill-harness.html"
            "?fail=pipeline-state-changed,audio-level,pipeline-error"
        )
        page.wait_for_selector('[data-testid="events-error"]')
        page.screenshot(path=str(OUT_DIR / "pill-status-unavailable.png"))

        context.close()
        browser.close()

    print(f"Wrote screenshots to {OUT_DIR}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://localhost:4173")
    args = parser.parse_args()
    capture(args.base_url)
