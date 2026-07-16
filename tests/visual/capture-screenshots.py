#!/usr/bin/env python3
"""Screenshot capture for the settings window's file-output picker (issue #180).

Drives `tests/visual/settings-harness.html` (Tauri IPC mocked via
`@tauri-apps/api/mocks`, see that file's doc comment) with Playwright's
Python bindings against a locally-running Vite dev server, per MISSION.md
§3's visual-verification method. Not a project dependency — Playwright is
a pre-provisioned system tool on the dev machine (see the PR description);
nothing here is imported by the shipped app or the Vitest/cargo suites.

Usage:
    python3 tests/visual/capture-screenshots.py [--base-url http://localhost:4173]

Writes PNGs into tests/visual/screenshots/ (kept out of the production
bundle — no Vite entry references that directory); the ones worth keeping
for PR review are copied under docs/design/screenshots/ separately.
"""

import argparse
import pathlib

from playwright.sync_api import sync_playwright

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
OUT_DIR = SCRIPT_DIR / "screenshots"


def capture(base_url: str) -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    with sync_playwright() as p:
        browser = p.chromium.launch()

        for scheme in ("light", "dark"):
            # `SettingsWindow` is a fixed `h-screen` layout with its own
            # internal `overflow-y-auto` panel (not page-level scroll), so
            # `full_page` screenshots can't capture anything the viewport
            # itself doesn't already fit — tall enough here to fit every
            # fixture's content (the File-mode fields add ~150px) without
            # relying on that internal scroll.
            context = browser.new_context(
                viewport={"width": 900, "height": 820},
                color_scheme=scheme,
            )
            page = context.new_page()

            # ---- default: Cursor mode, no file fields ----
            page.goto(f"{base_url}/tests/visual/settings-harness.html?fixture=default")
            page.wait_for_selector('[data-testid="general-panel"]')
            page.wait_for_selector('[data-testid="output-mode-cursor"]')
            page.screenshot(
                path=str(OUT_DIR / f"settings-output-default-{scheme}.png")
            )

            # ---- file mode selected: base folder + template fields shown ----
            page.goto(f"{base_url}/tests/visual/settings-harness.html?fixture=file-mode")
            page.wait_for_selector('[data-testid="file-output-fields"]')
            page.screenshot(
                path=str(OUT_DIR / f"settings-output-file-mode-{scheme}.png")
            )

            # ---- invalid-template error state: type an absolute path, blur ----
            template_input = page.locator('[data-testid="file-path-template-input"]')
            template_input.click()
            template_input.fill("/etc/passwd")
            page.locator('[data-testid="file-base-dir-input"]').click()  # blur the template field
            page.wait_for_selector('[data-testid="file-path-template-error"]')
            page.screenshot(
                path=str(OUT_DIR / f"settings-output-invalid-template-{scheme}.png"),
            )

            context.close()

        browser.close()

    print(f"Wrote screenshots to {OUT_DIR}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://localhost:4173")
    args = parser.parse_args()
    capture(args.base_url)
