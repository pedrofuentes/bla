# Learnings — bla

> **This file is written by AI agents.** When you discover something about this project
> that isn't documented elsewhere, add it here. Do NOT write to AGENTS.md.
>
> Periodically, a human or agent should review this file and promote stable learnings
> into the appropriate companion doc (ARCHITECTURE.md, TESTING-STRATEGY.md, etc.).

## Format

```markdown
### [YYYY-MM-DD] Short description
**Context**: What were you doing when you discovered this?
**Learning**: What did you learn?
**Impact**: How should this affect future work?
```

## Learnings

<!-- Add new learnings below this line, most recent first -->

### [2026-07-09] File-mode output paths use a literal `/`, never the host separator
**Context**: Windows runtime-seam hardening (#98/#100) and its Sentinel digest (#107, finding 3) — reviewing how `output`'s file-mode templating builds paths.
**Learning**: File-mode path templates (`output::expand_template`/`append_entry`, e.g. `{{date:YYYY-MM-DD}}.md` and nested `journal/{{date}}.md`) join segments with a **literal `/`**, deliberately NOT `std::path::MAIN_SEPARATOR` or `Path::join` on the host separator. This is intentional and locked in only by inline test comments in `output.rs` — the template is a user-authored, cross-platform-portable string, and Rust/Windows both accept `/` in paths, so a single convention keeps templated output identical across macOS and Windows and avoids leaking `\` into a user's configured template.
**Impact**: Do not "fix" file-mode path building to use host-separator logic (`Path::join` per segment, `MAIN_SEPARATOR`, backslashes on Windows) — that would be a regression, not a portability improvement. If this convention ever moves out of inline comments, promote it into `docs/ARCHITECTURE.md` (output module) rather than re-deriving it.
