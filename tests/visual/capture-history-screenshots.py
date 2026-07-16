#!/usr/bin/env python3
"""Screenshot capture for the settings window's History tab (issue #199).

Drives `tests/visual/settings-harness.html` (Tauri IPC mocked via
`@tauri-apps/api/mocks`, see that file's doc comment — including its
`?history=` fixture selector and the synthetic placeholder rows it seeds)
with Playwright's Python bindings against a locally-running Vite dev
server, per MISSION.md §3's visual-verification method. Not a project
dependency — Playwright is a pre-provisioned system tool on the dev
machine (see the PR description); nothing here is imported by the shipped
app or the Vitest/cargo suites.

Privacy (MISSION §5/§7): every history row rendered here comes from the
harness's `HISTORY_ROWS` fixture — obviously synthetic placeholder text,
never a real transcript.

Usage:
    python3 tests/visual/capture-history-screenshots.py [--base-url http://localhost:4173]

Writes PNGs into tests/visual/screenshots/ (kept out of the production
bundle — no Vite entry references that directory); the ones worth keeping
for PR review are copied under docs/design/screenshots/ separately.
"""

import argparse
import pathlib

from playwright.sync_api import sync_playwright

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
OUT_DIR = SCRIPT_DIR / "screenshots"


def goto_history_tab(page, base_url: str, history_fixture: str) -> None:
    page.goto(f"{base_url}/tests/visual/settings-harness.html?fixture=default&history={history_fixture}")
    page.wait_for_selector('[data-testid="general-panel"]')
    page.locator('[data-testid="tab-history"]').click()
    page.wait_for_selector('[data-testid="history-panel"]')


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

            # ---- default: a handful of entries ----
            goto_history_tab(page, base_url, "default")
            page.wait_for_selector('[data-testid^="history-row-"]')
            page.screenshot(path=str(OUT_DIR / f"settings-history-default-{scheme}.png"))

            # ---- empty state: no history yet ----
            goto_history_tab(page, base_url, "empty")
            page.wait_for_selector('[data-testid="history-empty-state"]')
            page.screenshot(path=str(OUT_DIR / f"settings-history-empty-{scheme}.png"))

            # ---- search-filtered: typing narrows the list ----
            goto_history_tab(page, base_url, "default")
            page.wait_for_selector('[data-testid^="history-row-"]')
            search = page.locator('[data-testid="history-search-input"]')
            search.click()
            search.fill("journal")
            page.wait_for_selector('[data-testid="history-row-1"]')
            page.screenshot(path=str(OUT_DIR / f"settings-history-search-filtered-{scheme}.png"))

            # ---- confirm-clear: inline confirm, not a native dialog ----
            goto_history_tab(page, base_url, "default")
            page.wait_for_selector('[data-testid^="history-row-"]')
            page.locator('[data-testid="history-clear-all-button"]').click()
            page.wait_for_selector('[data-testid="history-clear-confirm"]')
            page.screenshot(path=str(OUT_DIR / f"settings-history-confirm-clear-{scheme}.png"))

            context.close()

        browser.close()

    print(f"Wrote screenshots to {OUT_DIR}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://localhost:4173")
    args = parser.parse_args()
    capture(args.base_url)
