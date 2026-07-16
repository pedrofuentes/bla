/**
 * Client-side structural validation for the file-mode path template (issue
 * #180's settings-window picker).
 *
 * Mirrors the confinement rules `output::confine_relative_path` enforces
 * authoritatively at write time (`src-tauri/src/output.rs`, via
 * `output::route`): a template must be relative, and no `..` segment may
 * climb above the configured base folder at any point while walking it.
 * This is deliberately NOT a replacement for that Rust-side guard — every
 * dictation is confined regardless of what this lets through — it only lets
 * the picker reject an obviously-invalid template immediately, before it's
 * ever persisted, and show an inline error (AC-3's "invalid-template error
 * state").
 *
 * Operates on the raw (unexpanded) template string rather than an
 * already-resolved `{{date:...}}`/`{{time:...}}` value: those tokens never
 * themselves produce a leading `/` or a `..` component (see
 * `output::render_date_format`/`render_time_format`), so walking the
 * literal template's `/`-separated segments is equivalent, for confinement
 * purposes, to walking the fully-expanded path — including a token whose
 * own format string contains `/` (e.g. `{{date:YYYY/MM/DD}}`), whose pieces
 * just become ordinary path segments here too.
 */

export type PathTemplateValidation = { valid: true } | { valid: false; reason: string };

export function validatePathTemplate(template: string): PathTemplateValidation {
  if (template.trim().length === 0) {
    return { valid: false, reason: "Path template can't be empty." };
  }

  if (template.startsWith("/")) {
    return {
      valid: false,
      reason: "Path must be relative to the base folder — remove the leading \"/\".",
    };
  }

  let depth = 0;
  for (const segment of template.split("/")) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      depth -= 1;
      if (depth < 0) {
        return {
          valid: false,
          reason: "Path escapes the base folder — remove the \"..\" that climbs above it.",
        };
      }
    } else {
      depth += 1;
    }
  }

  return { valid: true };
}
