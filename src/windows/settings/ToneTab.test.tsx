import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { change, click, flush, mount, typeInto, type Mounted } from "../../testUtils";
import type { ToneRule } from "../../lib/ipc";
import { ToneTab } from "./ToneTab";

const invoke = vi.fn();

// `vi.mock` factories are hoisted above imports by vitest, so `ToneTab` above
// resolves against this mocked `../../lib/ipc` — the module under test never
// touches the real Tauri `invoke`.
vi.mock("../../lib/ipc", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

// Privacy (MISSION §5/§7): app patterns are user-environment data — every
// fixture pattern here is an obviously synthetic placeholder app name
// ("SynthMail" etc.), never a real installed app, and no test ever
// console.logs a pattern.
const RULE_A: ToneRule = {
  id: 1,
  app_pattern: "SynthMail",
  tone: "formal",
  created_at_ms: Date.parse("2026-07-10T09:00:00Z"),
};
const RULE_B: ToneRule = {
  id: 2,
  app_pattern: "ChatSynth*",
  tone: "casual",
  created_at_ms: Date.parse("2026-07-11T09:00:00Z"),
};

function setupInvoke(overrides: Partial<Record<string, (...args: unknown[]) => unknown>> = {}) {
  invoke.mockImplementation((command: string, args?: unknown) => {
    if (overrides[command]) return Promise.resolve(overrides[command]!(args));
    switch (command) {
      case "list_tone_rules":
        return Promise.resolve([RULE_A, RULE_B]);
      case "upsert_tone_rule":
        return Promise.resolve(3);
      case "delete_tone_rule":
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

async function addRule(container: HTMLElement, pattern: string, tone = "casual") {
  const input = container.querySelector<HTMLInputElement>(
    '[data-testid="tone-add-pattern-input"]',
  )!;
  typeInto(input, pattern);
  const select = container.querySelector<HTMLSelectElement>(
    '[data-testid="tone-add-tone-select"]',
  )!;
  change(select, tone);
  click(container.querySelector('[data-testid="tone-add-button"]')!);
  await flush();
}

describe("ToneTab (list rendering + match order)", () => {
  it("lists the rules returned by list_tone_rules on mount, in the order returned", async () => {
    mounted = mount(<ToneTab />);
    await flush();

    expect(invoke).toHaveBeenCalledWith("list_tone_rules");
    const items = Array.from(
      mounted.container.querySelectorAll('[data-testid^="tone-rule-pattern-"]'),
    );
    expect(items.map((el) => el.textContent)).toEqual([RULE_A.app_pattern, RULE_B.app_pattern]);
  });

  it("shows a loading state before the first list resolves", async () => {
    let resolveList!: (rows: ToneRule[]) => void;
    setupInvoke({
      list_tone_rules: () =>
        new Promise((resolve) => {
          resolveList = resolve;
        }),
    });

    mounted = mount(<ToneTab />);
    expect(mounted.container.querySelector('[data-testid="tone-loading"]')).not.toBeNull();

    resolveList([]);
    await flush();
    expect(mounted.container.querySelector('[data-testid="tone-loading"]')).toBeNull();
  });

  it("shows a kind-only inline error state when list_tone_rules rejects", async () => {
    setupInvoke({
      list_tone_rules: () => Promise.reject(new Error("some raw backend detail")),
    });

    mounted = mount(<ToneTab />);
    await flush();

    const error = mounted.container.querySelector('[data-testid="tone-load-error"]');
    expect(error).not.toBeNull();
    // Kind-only: the rendered message must NOT leak the raw backend error text.
    expect(error?.textContent).not.toMatch(/raw backend detail/);
  });

  it("shows an empty state when list_tone_rules returns no rows", async () => {
    setupInvoke({ list_tone_rules: () => [] });

    mounted = mount(<ToneTab />);
    await flush();

    expect(mounted.container.querySelector('[data-testid="tone-empty-state"]')).not.toBeNull();
  });
});

describe("ToneTab (AC-45: add a rule)", () => {
  it("submitting the add-rule form calls upsert_tone_rule and the new rule appears last in the rendered list", async () => {
    mounted = mount(<ToneTab />);
    await flush();

    await addRule(mounted.container, "TermSynth", "verbatim");

    expect(invoke).toHaveBeenCalledWith("upsert_tone_rule", {
      app_pattern: "TermSynth",
      tone: "verbatim",
    });
    const items = Array.from(
      mounted.container.querySelectorAll('[data-testid^="tone-rule-pattern-"]'),
    );
    expect(items.map((el) => el.textContent)).toEqual([
      RULE_A.app_pattern,
      RULE_B.app_pattern,
      "TermSynth",
    ]);
  });

  it("clears the add-rule pattern input after a successful add", async () => {
    mounted = mount(<ToneTab />);
    await flush();

    await addRule(mounted.container, "TermSynth");

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="tone-add-pattern-input"]',
    )!;
    expect(input.value).toBe("");
  });

  it("trims whitespace from a submitted pattern before calling upsert_tone_rule", async () => {
    mounted = mount(<ToneTab />);
    await flush();

    await addRule(mounted.container, "  TermSynth  ");

    expect(invoke).toHaveBeenCalledWith("upsert_tone_rule", {
      app_pattern: "TermSynth",
      tone: "casual",
    });
  });

  it("rejects an empty pattern submission with inline feedback and never calls upsert_tone_rule", async () => {
    mounted = mount(<ToneTab />);
    await flush();

    click(mounted.container.querySelector('[data-testid="tone-add-button"]')!);
    await flush();

    expect(invoke).not.toHaveBeenCalledWith("upsert_tone_rule", expect.anything());
    expect(mounted.container.querySelector('[data-testid="tone-add-error"]')).not.toBeNull();
  });

  it("rejects a whitespace-only pattern submission the same way", async () => {
    mounted = mount(<ToneTab />);
    await flush();

    await addRule(mounted.container, "   ");

    expect(invoke).not.toHaveBeenCalledWith("upsert_tone_rule", expect.anything());
    expect(mounted.container.querySelector('[data-testid="tone-add-error"]')).not.toBeNull();
  });

  it("rejects a case-insensitive duplicate of an existing pattern, without calling the backend", async () => {
    mounted = mount(<ToneTab />);
    await flush();
    invoke.mockClear();
    setupInvoke();

    // RULE_A's pattern is "SynthMail" — submit a differently-cased duplicate.
    await addRule(mounted.container, "synthmail");

    // Negative assertion (sibling-tab review lesson): the backend's own
    // upsert semantics never reject this — it silently UPDATEs the existing
    // row — so the tab must catch it client-side and never even place the
    // call, or it would render a phantom second row that doesn't exist
    // server-side.
    expect(invoke).not.toHaveBeenCalledWith("upsert_tone_rule", expect.anything());
    const error = mounted.container.querySelector('[data-testid="tone-add-error"]');
    expect(error).not.toBeNull();
    expect(error?.textContent).toMatch(/already/i);
    expect(mounted.container.querySelectorAll('[data-testid^="tone-rule-pattern-"]').length).toBe(
      2,
    );
  });

  it("disables the add button while the add call is in flight and re-enables it after", async () => {
    let resolveAdd!: (id: number) => void;
    setupInvoke({
      upsert_tone_rule: () =>
        new Promise((resolve) => {
          resolveAdd = resolve;
        }),
    });

    mounted = mount(<ToneTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="tone-add-pattern-input"]',
    )!;
    typeInto(input, "TermSynth");
    const button = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="tone-add-button"]',
    )!;
    click(button);
    await flush();

    expect(button.disabled).toBe(true);

    resolveAdd(9);
    await flush();

    expect(button.disabled).toBe(false);
  });

  it("shows a kind-only inline error and preserves the draft when upsert_tone_rule rejects on add", async () => {
    setupInvoke({
      upsert_tone_rule: () => Promise.reject(new Error("some raw backend detail")),
    });

    mounted = mount(<ToneTab />);
    await flush();

    await addRule(mounted.container, "Willfail");

    const error = mounted.container.querySelector('[data-testid="tone-add-error"]');
    expect(error).not.toBeNull();
    expect(error?.textContent).not.toMatch(/raw backend detail/);
    expect(
      Array.from(mounted.container.querySelectorAll('[data-testid^="tone-rule-pattern-"]')).some(
        (el) => el.textContent === "Willfail",
      ),
    ).toBe(false);
    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="tone-add-pattern-input"]',
    )!;
    expect(input.value).toBe("Willfail");
  });
});

describe("ToneTab (AC-45: remove a rule)", () => {
  it("removing a rule calls delete_tone_rule and it disappears from the rendered list", async () => {
    mounted = mount(<ToneTab />);
    await flush();

    click(mounted.container.querySelector('[data-testid="tone-rule-remove-1"]')!);
    await flush();

    expect(invoke).toHaveBeenCalledWith("delete_tone_rule", { id: 1 });
    expect(mounted.container.querySelector('[data-testid="tone-rule-1"]')).toBeNull();
    expect(mounted.container.querySelector('[data-testid="tone-rule-2"]')).not.toBeNull();
  });

  it("disables the remove button while the delete call is in flight", async () => {
    let resolveDelete!: () => void;
    setupInvoke({
      delete_tone_rule: () =>
        new Promise((resolve) => {
          resolveDelete = () => resolve(undefined);
        }),
    });

    mounted = mount(<ToneTab />);
    await flush();

    const button = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="tone-rule-remove-1"]',
    )!;
    click(button);
    await flush();

    expect(button.disabled).toBe(true);
    // Still present (only removed from the list once the call resolves).
    expect(mounted.container.querySelector('[data-testid="tone-rule-1"]')).not.toBeNull();

    resolveDelete();
    await flush();

    // Resolved successfully: the row (and its button) is gone entirely
    // rather than re-enabled — there is no "re-enabled" state to assert for
    // a control whose success path unmounts it.
    expect(mounted.container.querySelector('[data-testid="tone-rule-1"]')).toBeNull();
  });

  it("re-enables the remove button after a failed delete call", async () => {
    let resolveDelete!: (err: Error) => void;
    setupInvoke({
      delete_tone_rule: () =>
        new Promise((_resolve, reject) => {
          resolveDelete = reject;
        }),
    });

    mounted = mount(<ToneTab />);
    await flush();

    const button = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="tone-rule-remove-1"]',
    )!;
    click(button);
    await flush();

    expect(button.disabled).toBe(true);

    resolveDelete(new Error("some raw backend detail"));
    await flush();

    expect(button.disabled).toBe(false);
    expect(mounted.container.querySelector('[data-testid="tone-rule-1"]')).not.toBeNull();
  });

  it("keeps the rule in the list and shows a kind-only inline error when delete_tone_rule rejects", async () => {
    setupInvoke({
      delete_tone_rule: () => Promise.reject(new Error("some raw backend detail")),
    });

    mounted = mount(<ToneTab />);
    await flush();

    click(mounted.container.querySelector('[data-testid="tone-rule-remove-1"]')!);
    await flush();

    expect(mounted.container.querySelector('[data-testid="tone-rule-1"]')).not.toBeNull();
    const error = mounted.container.querySelector('[data-testid="tone-remove-error"]');
    expect(error).not.toBeNull();
    expect(error?.textContent).not.toMatch(/raw backend detail/);
  });
});

describe("ToneTab (AC-44: edit a rule's tone)", () => {
  it("changing a rule's tone select calls upsert_tone_rule with its pattern and the new tone, reflected immediately", async () => {
    mounted = mount(<ToneTab />);
    await flush();

    const select = mounted.container.querySelector<HTMLSelectElement>(
      '[data-testid="tone-rule-tone-select-1"]',
    )!;
    expect(select.value).toBe("formal");

    change(select, "casual");
    await flush();

    expect(invoke).toHaveBeenCalledWith("upsert_tone_rule", {
      app_pattern: "SynthMail",
      tone: "casual",
    });
    expect(select.value).toBe("casual");
  });

  it("disables the tone select while the edit call is in flight and re-enables it after", async () => {
    let resolveEdit!: (id: number) => void;
    setupInvoke({
      upsert_tone_rule: () =>
        new Promise((resolve) => {
          resolveEdit = resolve;
        }),
    });

    mounted = mount(<ToneTab />);
    await flush();

    const select = mounted.container.querySelector<HTMLSelectElement>(
      '[data-testid="tone-rule-tone-select-1"]',
    )!;
    change(select, "casual");
    await flush();

    expect(select.disabled).toBe(true);

    resolveEdit(1);
    await flush();

    expect(select.disabled).toBe(false);
  });

  it("reverts the select to the previous tone and shows a kind-only inline error when the edit call rejects", async () => {
    setupInvoke({
      upsert_tone_rule: () => Promise.reject(new Error("some raw backend detail")),
    });

    mounted = mount(<ToneTab />);
    await flush();

    const select = mounted.container.querySelector<HTMLSelectElement>(
      '[data-testid="tone-rule-tone-select-1"]',
    )!;
    change(select, "casual");
    await flush();

    expect(select.value).toBe("formal");
    const error = mounted.container.querySelector('[data-testid="tone-rule-edit-error-1"]');
    expect(error).not.toBeNull();
    expect(error?.textContent).not.toMatch(/raw backend detail/);
  });

  it("only applies the response from the latest of two rapid, out-of-order edits to the same rule", async () => {
    const resolvers: Array<(id: number) => void> = [];
    setupInvoke({
      upsert_tone_rule: () =>
        new Promise((resolve) => {
          resolvers.push(resolve);
        }),
    });

    mounted = mount(<ToneTab />);
    await flush();

    const select = mounted.container.querySelector<HTMLSelectElement>(
      '[data-testid="tone-rule-tone-select-1"]',
    )!;

    // First edit: formal -> casual.
    change(select, "casual");
    await flush();
    // Second edit fired before the first's response arrives: casual -> verbatim.
    change(select, "verbatim");
    await flush();

    expect(resolvers).toHaveLength(2);

    // Resolve OUT OF ORDER: the second (later) request settles first, then
    // the first (now-stale) request settles after it.
    resolvers[1](2);
    await flush();
    resolvers[0](1);
    await flush();

    // The stale first response must not clobber the later selection.
    expect(select.value).toBe("verbatim");
  });
});
