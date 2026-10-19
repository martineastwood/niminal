//! The niminal engine: a sound server. It holds the instruments, effects, tracks, buses and controls that
//! niminal code defines, and plays timestamped events (notes, parameter changes, control changes,
//! definitions) sent to it through a public API. It knows nothing of clips, scenes or patterns:
//! those are for the sequencer, which is one client among others.

mod audio_in;
mod clock;
mod controls;
mod engine_server;
mod events;
mod log;
mod midi_out;
mod resources;
mod routing;
pub use routing::{Endpoint, RoutingEdit};
mod session;
mod sounding;
mod status;

pub use audio_in::AudioInput;
pub use clock::ClockProbe;
pub use engine_server::{
    ClientId, ENGINE_TOPICS, EngineError, EngineReply, EngineRequest, EngineServer, PROTOCOL_VERSION, clock_json,
    problem_json,
};
pub use events::{At, Cancelled, Op, Sent, TransactionReceipt};
pub use log::{LogEntry, LogInput, from_lines, to_lines};
pub use midi_out::MidiEvent;
pub use niminal_lang::{Entry, Located, Origin, Piece, Problem, Project};
pub use session::{ClockInfo, Session, EngineState};
pub use status::status_json;

pub use session::realtime::{RealtimeConfig, RealtimeRenderer, RenderMetrics};
