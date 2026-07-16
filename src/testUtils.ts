/**
 * Minimal render/event helpers for component tests, standing in for
 * `@testing-library/react` (not a project dependency — see
 * `docs/DEVELOPMENT-WORKFLOW.md`/AGENTS.md "ASK FIRST: adding dependencies").
 * Drives real DOM nodes with `react-dom/client` + React's `act`, which is
 * enough for the pill/settings windows' plain-element markup (no
 * portals/refs beyond what `act` already flushes).
 */
import { act } from "react";
import type { ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";

// React 19's `act` warns unless the environment declares itself act-aware;
// jsdom (this project's vitest `environment`) doesn't set this itself.
(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

export interface Mounted {
  container: HTMLDivElement;
  root: Root;
  unmount: () => void;
}

/** Mounts `ui` into a fresh, attached container, flushing the initial render. */
export function mount(ui: ReactElement): Mounted {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  act(() => {
    root.render(ui);
  });
  return {
    container,
    root,
    unmount: () => {
      act(() => {
        root.unmount();
      });
      container.remove();
    },
  };
}

/** Flushes pending microtasks (resolved promises from mocked `invoke`/`onEvent`) under `act`. */
export async function flush(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

/** Dispatches a bubbling click, wrapped in `act`. */
export function click(el: Element): void {
  act(() => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

/** Focuses `el`, wrapped in `act`. */
export function focus(el: HTMLElement): void {
  act(() => {
    el.focus();
  });
}

/** Blurs `el`, wrapped in `act`. */
export function blur(el: HTMLElement): void {
  act(() => {
    el.blur();
  });
}

/** Dispatches a bubbling keydown, wrapped in `act`. */
export function keydown(el: Element, key: string, mods: KeyboardEventInit = {}): void {
  act(() => {
    el.dispatchEvent(
      new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...mods }),
    );
  });
}

/** Sets a form control's value and dispatches `change`, wrapped in `act`. */
export function change(el: HTMLInputElement | HTMLSelectElement, value: string): void {
  act(() => {
    el.value = value;
    el.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

/**
 * Sets a text `<input>`'s value and dispatches `input`, wrapped in `act`.
 * React's `onChange` for text-like inputs listens to the native `input`
 * event, not `change` (issue #180's path/folder text fields) — `change()`
 * above works for `<select>`/checkbox/radio, but a text field's onChange
 * handler never fires from a bare `change` event.
 */
export function typeInto(el: HTMLInputElement, value: string): void {
  act(() => {
    el.value = value;
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
}
