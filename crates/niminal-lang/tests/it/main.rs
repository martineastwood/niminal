//! The small integration tests of this crate, built as one binary to keep linking quick.
#![allow(clippy::unwrap_used)]
mod arrays;
mod blocks;
mod bundled_samples;
mod composition;
mod control_scopes;
mod frames;
mod fx_helpers;
mod granular;
mod instrument_scopes;
mod kernel_optimizations;
mod lanes;
mod language_version;
mod mverb;
mod oscillators;
mod physical;
mod prelude_math;
mod preset_quality;
mod reserved_names;
mod sfz;
mod spectral;
mod spectral_tools;
mod stdlib;
mod stretch;
mod tables;
mod two_compilers;
mod unread_settings;
