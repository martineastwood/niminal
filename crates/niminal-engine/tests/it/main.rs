//! The small integration tests of this crate, built as one binary to keep linking quick.
#![allow(clippy::unwrap_used)]
mod common;
mod alias_quality;
mod arrays;
mod custom;
mod denormals;
mod dsp_quality;
mod effects_quality;
mod env;
mod env_one_sample;
mod filter_quality;
mod graph_pruning;
mod interpreter;
mod mixer;
mod osc_filter;
mod physical;
mod pitch_glides;
mod scalar_fusion;
mod time_effects;
mod voice;
