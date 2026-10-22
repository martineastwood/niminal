//! niminal's DSP runtime.
//!
//! The engine deals only in plain numbers: frequencies in hz, gains as linear
//! factors, times in seconds. Units (`800hz`, `-6db`, `1/8beat`) are the
//! business of the language and score layers, which convert before building a
//! graph. There is no OS-specific code here, so the crate builds for WebAssembly.

mod activity;
mod array;
pub use activity::{Activity, WAITING_SAMPLE};
pub mod denormals;
mod graph;
mod mapping;
mod master;
pub use mapping::ParamMapping;
mod mixer;
mod op_dispatch;
mod opcode;
pub mod ops;
mod table;
mod voice;
mod waveform;
pub use waveform::{WAVEFORM_FRAMES, Waveform};

pub use array::{Array, wrapped_index};
pub use graph::{Graph, GraphBuilder, GraphError, NodeReport, Src};
pub use master::{DEFAULT_CEILING, Master};
pub use mixer::{
    Buses, ChainInput, DEFAULT_MAX_VOICES, Fork, MAX_CHANNELS, Meter, Mixer, NodeDef, NodeLoad,
    Profile, Route, Stems, Transfer, VoiceId, execution_order,
};
pub use opcode::{
    Clock, DEFAULT_BLOCK, DEFAULT_CONTROL_STEP, KernelStats, MAX_BLOCK, MAX_INPUTS, MIN_BLOCK, Opcode, Port, ProcessCtx, TAIL_LEVEL,
    block_size, blocks, control_step, set_block_size, set_control_step,
};
pub use table::{Buffer, Edge, Table};
pub use voice::{Priority, Voice, VoiceRule};

/// Stable identity for an fx source across prepared graph replacements.
pub fn instance_id(name: &str) -> u64 {
    name.bytes().fold(14695981039346656037u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(1099511628211)
    })
}

pub type BufferBank = std::sync::Arc<Vec<std::sync::Arc<Buffer>>>;
