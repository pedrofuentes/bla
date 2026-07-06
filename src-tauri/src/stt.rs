//! Speech-to-text via `whisper-rs` (whisper.cpp bindings), Metal-accelerated on macOS.
//!
//! Transcribes the audio buffer produced by `audio`. Personal-dictionary terms
//! (from `store`) are passed as Whisper's `initial_prompt` to bias recognition
//! toward the user's vocabulary.
//!
//! Pure-logic-adjacent: the whisper.cpp call is native glue, but pre/post
//! processing (prompt construction, output normalization) should stay unit-testable.
//!
//! Stub — no logic yet; implemented in a later M1 increment.
