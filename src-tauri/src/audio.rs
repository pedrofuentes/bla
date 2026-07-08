//! Audio capture: `cpal` input stream → 16 kHz mono `f32` ring buffer.
//!
//! Opens the default input device, resamples/downmixes to the format `stt`
//! expects, and buffers samples for the duration of a hold-to-record session.
//!
//! OS-integration module (AGENTS.md §OS-integration exemption): thin glue only —
//! no cleanup/decision logic lives here.
//!
//! Stub — no logic yet; implemented in a later M1 increment.
