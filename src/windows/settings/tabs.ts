/**
 * Settings window tab definitions (issue #126, M2 PR 2.5).
 *
 * A single source of truth for the tab bar so `index.tsx` doesn't hardcode
 * tab IDs/labels twice (bar + panel switch). `"general"` (#126), `"history"`
 * (#199), `"dictionary"` (#201), and `"tone"` (#203) have real content;
 * `"snippets"` still renders a shared "coming soon" placeholder (see
 * `index.tsx`) so the tab bar's final shape is in place without pulling
 * forward its actual UI ahead of its own M3+ increment.
 */
export type TabId = "general" | "history" | "dictionary" | "tone" | "snippets";

export interface TabDef {
  id: TabId;
  label: string;
}

export const TABS: readonly TabDef[] = [
  { id: "general", label: "General" },
  { id: "history", label: "History" },
  { id: "dictionary", label: "Dictionary" },
  { id: "tone", label: "Tone" },
  { id: "snippets", label: "Snippets" },
];
