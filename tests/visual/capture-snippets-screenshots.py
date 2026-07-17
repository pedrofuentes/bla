#!/usr/bin/env python3
"""Screenshot capture for the settings window's Snippets tab (issue #261).

Drives `tests/visual/settings-harness.html` (Tauri IPC mocked via
`@tauri-apps/api/mocks`, see that file's doc comment — including its
`?snippets=` fixture selector and the synthetic placeholder triggers/bodies
it seeds) with Playwright's Python bindings against a locally-running Vite
dev server, per MISSION.md §3's visual-verification method. Not a project
dependency — Playwright is a pre-provisioned system tool on the dev
machine (see the PR description); nothing here is imported by the shipped
app or the Vitest/cargo suites.

Privacy (MISSION §5/§7): every trigger/body rendered here comes from the
harness's `SNIPPETS` fixture — obviously synthetic placeholder text, never
a real dictated snippet.

Usage:
    python3 tests/visual/capture-snippets-screenshots.py [--base-url http://localhost:4173]

Writes PNGs into tests/visual/screenshots/ (kept out of the production
bundle — no Vite entry references that directory); the ones worth keeping
for PR review are copied under docs/design/screenshots/ separately.
"""

import argparse
import pathlib

from playwright.sync_api import sync_playwright

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
OUT_DIR = SCRIPT_DIR / "screenshots"


def goto_snippets_tab(page, base_url: str, snippets_fixture: str) -> None:
    page.goto(
        f"{base_url}/tests/visual/settings-harness.html?fixture=default&snippets={snippets_fixture}"
    )
    page.wait_for_selector('[data-testid="general-panel"]')
    page.locator('[data-testid="tab-snippets"]').click()
    page.wait_for_selector('[data-testid="snippets-panel"]')


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

            # ---- default: a handful of trigger/body entries ----
            goto_snippets_tab(page, base_url, "default")
            page.wait_for_selector('[data-testid^="snippet-trigger-"]')
            page.screenshot(path=str(OUT_DIR / f"settings-snippets-default-{scheme}.png"))

            # ---- empty state: no snippets yet ----
            goto_snippets_tab(page, base_url, "empty")
            page.wait_for_selector('[data-testid="snippets-empty-state"]')
            page.screenshot(path=str(OUT_DIR / f"settings-snippets-empty-{scheme}.png"))

            # ---- add-in-progress: a new trigger/body typed, not yet submitted ----
            goto_snippets_tab(page, base_url, "default")
            page.wait_for_selector('[data-testid^="snippet-trigger-"]')
            add_trigger = page.locator('[data-testid="snippets-add-trigger-input"]')
            add_trigger.click()
            add_trigger.fill("brb")
            add_body = page.locator('[data-testid="snippets-add-body-input"]')
            add_body.click()
            add_body.fill("Placeholder — be right back.")
            page.screenshot(path=str(OUT_DIR / f"settings-snippets-add-in-progress-{scheme}.png"))

            # ---- validation error: submitting a case-insensitive duplicate trigger ----
            goto_snippets_tab(page, base_url, "default")
            page.wait_for_selector('[data-testid^="snippet-trigger-"]')
            dup_trigger = page.locator('[data-testid="snippets-add-trigger-input"]')
            dup_trigger.click()
            dup_trigger.fill("SIG")
            dup_body = page.locator('[data-testid="snippets-add-body-input"]')
            dup_body.click()
            dup_body.fill("Some other placeholder body")
            page.locator('[data-testid="snippets-add-button"]').click()
            page.wait_for_selector('[data-testid="snippets-add-error"]')
            page.screenshot(
                path=str(OUT_DIR / f"settings-snippets-validation-error-{scheme}.png")
            )

            context.close()

        browser.close()

    print(f"Wrote screenshots to {OUT_DIR}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://localhost:4173")
    args = parser.parse_args()
    capture(args.base_url)
