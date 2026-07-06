//! Output router: the only path that writes recognized text somewhere.
//!
//! Two targets: clipboard-swap + synthesized paste (`enigo`) into the focused
//! app, or templated append to a Markdown file (`{{date:YYYY-MM-DD}}` path
//! templating, optional timestamps — the Obsidian daily-note flow).
//!
//! OS-integration module (AGENTS.md §OS-integration exemption) for the paste
//! path; path templating itself should stay pure-logic and unit-testable.
//! Never logs or persists raw clipboard contents (MISSION §5).
//!
//! Stub — no logic yet; implemented in a later M1 increment.
