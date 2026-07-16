import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { click, flush, mount, typeInto, type Mounted } from "../../testUtils";
import type { DictionaryTerm } from "../../lib/ipc";
import { DictionaryTab } from "./DictionaryTab";

const invoke = vi.fn();

// `vi.mock` factories are hoisted above imports by vitest, so `DictionaryTab`
// above resolves against this mocked `../../lib/ipc` — the module under
// test never touches the real Tauri `invoke`.
vi.mock("../../lib/ipc", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

// Privacy (MISSION §5/§7): every fixture term here is an obviously
// synthetic placeholder — no real user vocabulary anywhere in this file,
// and no test ever console.logs a term.
const TERM_A: DictionaryTerm = {
  id: 1,
  term: "Fixturon",
  created_at_ms: Date.parse("2026-07-10T09:00:00Z"),
};
const TERM_B: DictionaryTerm = {
  id: 2,
  term: "synthetiql",
  created_at_ms: Date.parse("2026-07-11T09:00:00Z"),
};

function setupInvoke(overrides: Partial<Record<string, (...args: unknown[]) => unknown>> = {}) {
  invoke.mockImplementation((command: string, args?: unknown) => {
    if (overrides[command]) return Promise.resolve(overrides[command]!(args));
    switch (command) {
      case "list_dictionary_terms":
        return Promise.resolve([TERM_B, TERM_A]);
      case "add_dictionary_term":
        return Promise.resolve(3);
      case "remove_dictionary_term":
        return Promise.resolve(undefined);
      default:
        return Promise.reject(new Error(`unmocked command ${command}`));
    }
  });
}

let mounted: Mounted | undefined;

beforeEach(() => {
  invoke.mockReset();
  setupInvoke();
});

afterEach(() => {
  mounted?.unmount();
  mounted = undefined;
});

async function addTerm(container: HTMLElement, term: string) {
  const input = container.querySelector<HTMLInputElement>('[data-testid="dictionary-add-input"]')!;
  typeInto(input, term);
  click(container.querySelector('[data-testid="dictionary-add-button"]')!);
  await flush();
}

describe("DictionaryTab (AC-38: list/add/remove)", () => {
  it("lists the terms returned by list_dictionary_terms on mount", async () => {
    mounted = mount(<DictionaryTab />);
    await flush();

    expect(invoke).toHaveBeenCalledWith("list_dictionary_terms");
    expect(mounted.container.querySelector('[data-testid="dictionary-term-1"]')).not.toBeNull();
    expect(mounted.container.querySelector('[data-testid="dictionary-term-2"]')).not.toBeNull();
    expect(
      mounted.container.querySelector('[data-testid="dictionary-term-label-1"]')?.textContent,
    ).toBe(TERM_A.term);
  });

  it("shows a loading state before the first list resolves", async () => {
    let resolveList!: (rows: DictionaryTerm[]) => void;
    setupInvoke({
      list_dictionary_terms: () =>
        new Promise((resolve) => {
          resolveList = resolve;
        }),
    });

    mounted = mount(<DictionaryTab />);
    expect(mounted.container.querySelector('[data-testid="dictionary-loading"]')).not.toBeNull();

    resolveList([]);
    await flush();
    expect(mounted.container.querySelector('[data-testid="dictionary-loading"]')).toBeNull();
  });

  it("shows a kind-only inline error state when list_dictionary_terms rejects", async () => {
    setupInvoke({
      list_dictionary_terms: () => Promise.reject(new Error("some raw backend detail")),
    });

    mounted = mount(<DictionaryTab />);
    await flush();

    const error = mounted.container.querySelector('[data-testid="dictionary-load-error"]');
    expect(error).not.toBeNull();
    // Kind-only: the rendered message must NOT leak the raw backend error text.
    expect(error?.textContent).not.toMatch(/raw backend detail/);
  });

  it("shows an empty state when list_dictionary_terms returns no rows", async () => {
    setupInvoke({ list_dictionary_terms: () => [] });

    mounted = mount(<DictionaryTab />);
    await flush();

    expect(
      mounted.container.querySelector('[data-testid="dictionary-empty-state"]'),
    ).not.toBeNull();
  });

  it("submitting the add-term input calls add_dictionary_term and the new term appears in the rendered list", async () => {
    mounted = mount(<DictionaryTab />);
    await flush();

    await addTerm(mounted.container, "Newterminal");

    expect(invoke).toHaveBeenCalledWith("add_dictionary_term", { term: "Newterminal" });
    expect(mounted.container.querySelector('[data-testid="dictionary-term-3"]')).not.toBeNull();
    expect(
      mounted.container.querySelector('[data-testid="dictionary-term-label-3"]')?.textContent,
    ).toBe("Newterminal");
  });

  it("clears the add-term input after a successful add", async () => {
    mounted = mount(<DictionaryTab />);
    await flush();

    await addTerm(mounted.container, "Newterminal");

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="dictionary-add-input"]',
    )!;
    expect(input.value).toBe("");
  });

  it("trims whitespace from a submitted term before calling add_dictionary_term", async () => {
    mounted = mount(<DictionaryTab />);
    await flush();

    await addTerm(mounted.container, "  Padded  ");

    expect(invoke).toHaveBeenCalledWith("add_dictionary_term", { term: "Padded" });
  });

  it("removing a term calls remove_dictionary_term and it disappears from the rendered list", async () => {
    mounted = mount(<DictionaryTab />);
    await flush();

    click(mounted.container.querySelector('[data-testid="dictionary-remove-1"]')!);
    await flush();

    expect(invoke).toHaveBeenCalledWith("remove_dictionary_term", { id: 1 });
    expect(mounted.container.querySelector('[data-testid="dictionary-term-1"]')).toBeNull();
    expect(mounted.container.querySelector('[data-testid="dictionary-term-2"]')).not.toBeNull();
  });
});

describe("DictionaryTab (AC-39: add-term validation)", () => {
  it("rejects an empty submission with inline feedback and never calls add_dictionary_term", async () => {
    mounted = mount(<DictionaryTab />);
    await flush();

    click(mounted.container.querySelector('[data-testid="dictionary-add-button"]')!);
    await flush();

    expect(invoke).not.toHaveBeenCalledWith("add_dictionary_term", expect.anything());
    expect(mounted.container.querySelector('[data-testid="dictionary-add-error"]')).not.toBeNull();
  });

  it("rejects a whitespace-only submission the same way", async () => {
    mounted = mount(<DictionaryTab />);
    await flush();

    await addTerm(mounted.container, "   ");

    expect(invoke).not.toHaveBeenCalledWith("add_dictionary_term", expect.anything());
    expect(mounted.container.querySelector('[data-testid="dictionary-add-error"]')).not.toBeNull();
  });

  it("surfaces inline validation for a case-insensitive duplicate of an existing term, without calling the backend", async () => {
    mounted = mount(<DictionaryTab />);
    await flush();
    invoke.mockClear();
    setupInvoke();

    // TERM_A is "Fixturon" — submit a differently-cased duplicate.
    await addTerm(mounted.container, "fixturon");

    // Negative assertion (PR #223 review lesson): the backend's own
    // UNIQUE COLLATE NOCASE constraint never rejects this call — it's a
    // silent INSERT OR IGNORE no-op — so the tab must catch it client-side
    // and never even place the call.
    expect(invoke).not.toHaveBeenCalledWith("add_dictionary_term", expect.anything());
    const error = mounted.container.querySelector('[data-testid="dictionary-add-error"]');
    expect(error).not.toBeNull();
    expect(error?.textContent).toMatch(/already/i);
    // No duplicate row was added.
    expect(
      mounted.container.querySelectorAll('[data-testid^="dictionary-term-label-"]').length,
    ).toBe(2);
  });

  it("clears a stale validation error once the user edits the input again", async () => {
    mounted = mount(<DictionaryTab />);
    await flush();

    click(mounted.container.querySelector('[data-testid="dictionary-add-button"]')!);
    await flush();
    expect(mounted.container.querySelector('[data-testid="dictionary-add-error"]')).not.toBeNull();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="dictionary-add-input"]',
    )!;
    typeInto(input, "N");
    await flush();

    expect(mounted.container.querySelector('[data-testid="dictionary-add-error"]')).toBeNull();
  });
});

describe("DictionaryTab (failure/revert branches)", () => {
  it("shows a kind-only inline error and leaves the input intact when add_dictionary_term rejects", async () => {
    setupInvoke({
      add_dictionary_term: () => Promise.reject(new Error("some raw backend detail")),
    });

    mounted = mount(<DictionaryTab />);
    await flush();

    await addTerm(mounted.container, "Willfail");

    const error = mounted.container.querySelector('[data-testid="dictionary-add-error"]');
    expect(error).not.toBeNull();
    expect(error?.textContent).not.toMatch(/raw backend detail/);
    // The term was not optimistically added to the list.
    expect(
      Array.from(
        mounted.container.querySelectorAll('[data-testid^="dictionary-term-label-"]'),
      ).some((el) => el.textContent === "Willfail"),
    ).toBe(false);
    // The draft input is preserved so the user doesn't lose what they typed.
    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="dictionary-add-input"]',
    )!;
    expect(input.value).toBe("Willfail");
  });

  it("keeps the term in the list and shows a kind-only inline error when remove_dictionary_term rejects", async () => {
    setupInvoke({
      remove_dictionary_term: () => Promise.reject(new Error("some raw backend detail")),
    });

    mounted = mount(<DictionaryTab />);
    await flush();

    click(mounted.container.querySelector('[data-testid="dictionary-remove-1"]')!);
    await flush();

    expect(mounted.container.querySelector('[data-testid="dictionary-term-1"]')).not.toBeNull();
    const error = mounted.container.querySelector('[data-testid="dictionary-remove-error"]');
    expect(error).not.toBeNull();
    expect(error?.textContent).not.toMatch(/raw backend detail/);
  });
});
