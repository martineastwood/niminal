//! The niminal engine: an audio server. It holds the instruments, effects, nodes, buses and controls that
//! niminal code defines, and plays timestamped events (notes, parameter changes, control changes,
//! definitions) sent to it through a public API. It knows nothing of clips, scenes or patterns:
//! those are for the sequencer, which is one client among others.

mod audio_control;
mod audio_in;
pub use audio_control::{AudioControl, AudioSettings};
mod activity;
mod clock;
mod controls;
mod engine_server;
mod events;
mod files;
mod jobs;
mod log;
mod midi_out;
mod pool;
mod resources;
mod streaming;
mod wav;
pub use jobs::{Output as RenderOutput, RenderSpec, Resource as RenderResource};
pub use streaming::Player;
mod routing;
pub use routing::{Endpoint, ModSource, RoutingEdit};
mod reports;
mod scopes;
mod session;
mod sounding;
mod status;

pub use audio_in::AudioInput;
pub use clock::ClockProbe;
pub use engine_server::{
    ClientId, ENGINE_TOPICS, EngineError, EngineReply, EngineRequest, EngineServer,
    PROTOCOL_VERSION, clock_json, problem_json,
};
pub use events::{At, Cancelled, Op, Sent, TransactionReceipt};
pub use log::{LogEntry, LogInput, from_lines, to_lines};
pub use midi_out::MidiEvent;
pub use niminal_lang::{Entry, Located, Origin, Piece, Problem, Project};
pub use pool::{Policy, Pool};
pub use session::{ClockInfo, EngineState, Session};
pub use status::status_json;

pub use session::realtime::{RealtimeConfig, RealtimeRenderer, RenderMetrics};
