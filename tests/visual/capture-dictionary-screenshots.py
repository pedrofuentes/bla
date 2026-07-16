#!/usr/bin/env python3
"""Screenshot capture for the settings window's Dictionary tab (issue #201).

Drives `tests/visual/settings-harness.html` (Tauri IPC mocked via
`@tauri-apps/api/mocks`, see that file's doc comment — including its
`?dictionary=` fixture selector and the synthetic placeholder terms it
seeds) with Playwright's Python bindings against a locally-running Vite
dev server, per MISSION.md §3's visual-verification method. Not a project
dependency — Playwright is a pre-provisioned system tool on the dev
machine (see the PR description); nothing here is imported by the shipped
app or the Vitest/cargo suites.

Privacy (MISSION §5/§7): every term rendered here comes from the harness's
`DICTIONARY_TERMS` fixture — obviously synthetic placeholder text, never a
real personal-dictionary entry.

Usage:
    python3 tests/visual/capture-dictionary-screenshots.py [--base-url http://localhost:4173]

Writes PNGs into tests/visual/screenshots/ (kept out of the production
bundle — no Vite entry references that directory); the ones worth keeping
for PR review are copied under docs/design/screenshots/ separately.
"""

import argparse
import pathlib

from playwright.sync_api import sync_playwright

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
OUT_DIR = SCRIPT_DIR / "screenshots"


def goto_dictionary_tab(page, base_url: str, dictionary_fixture: str) -> None:
    page.goto(
        f"{base_url}/tests/visual/settings-harness.html?fixture=default&dictionary={dictionary_fixture}"
    )
    page.wait_for_selector('[data-testid="general-panel"]')
    page.locator('[data-testid="tab-dictionary"]').click()
    page.wait_for_selector('[data-testid="dictionary-panel"]')


def capture(base_url: str) -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    with sync_playwright() as p:
        browser = p.chromium.launch()

        for scheme in ("light", "dark"):
            context = browser.new_context(
                viewport={"width": 900, "height": 820},
                color_scheme=scheme,
            )
            page = context.new_page()

            # ---- default: a handful of terms ----
            goto_dictionary_tab(page, base_url, "default")
            page.wait_for_selector('[data-testid^="dictionary-term-"]')
            page.screenshot(path=str(OUT_DIR / f"settings-dictionary-default-{scheme}.png"))

            # ---- empty state: no terms yet ----
            goto_dictionary_tab(page, base_url, "empty")
            page.wait_for_selector('[data-testid="dictionary-empty-state"]')
            page.screenshot(path=str(OUT_DIR / f"settings-dictionary-empty-{scheme}.png"))

            # ---- add-in-progress: a new term typed, not yet submitted ----
            goto_dictionary_tab(page, base_url, "default")
            page.wait_for_selector('[data-testid^="dictionary-term-"]')
            add_input = page.locator('[data-testid="dictionary-add-input"]')
            add_input.click()
            add_input.fill("Newplaceholder")
            page.screenshot(path=str(OUT_DIR / f"settings-dictionary-add-in-progress-{scheme}.png"))

            # ---- validation error: submitting a case-insensitive duplicate ----
            goto_dictionary_tab(page, base_url, "default")
            page.wait_for_selector('[data-testid^="dictionary-term-"]')
            dup_input = page.locator('[data-testid="dictionary-add-input"]')
            dup_input.click()
            dup_input.fill("fixtureon")
            page.locator('[data-testid="dictionary-add-button"]').click()
            page.wait_for_selector('[data-testid="dictionary-add-error"]')
            page.screenshot(
                path=str(OUT_DIR / f"settings-dictionary-validation-error-{scheme}.png")
            )

            context.close()

        browser.close()

    print(f"Wrote screenshots to {OUT_DIR}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://localhost:4173")
    args = parser.parse_args()
    capture(args.base_url)
