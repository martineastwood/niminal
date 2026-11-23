//! The small integration tests of this crate, built as one binary to keep linking quick.
#![allow(clippy::unwrap_used)]
#[path = "../support/mod.rs"]
mod support;
mod controls;
mod engine_protocol;
mod events;
mod frontends;
mod fx;
mod hostile_programs;
mod instrument_controls;
mod pitch_glides;
mod routing;
