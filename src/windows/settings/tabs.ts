/**
 * Settings window tab definitions (issue #126, M2 PR 2.5).
 *
 * A single source of truth for the tab bar so `index.tsx` doesn't hardcode
 * tab IDs/labels twice (bar + panel switch). `"general"` (#126), `"history"`
 * (#199), and `"dictionary"` (#201) have real content; `"tone"` and
 * `"snippets"` still render a shared "coming soon" placeholder (see
 * `index.tsx`) so the tab bar's final shape is in place without pulling
 * forward their actual UI ahead of their own M3+ increments.
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
