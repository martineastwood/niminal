//! The niminal live session: quantized, transactional evaluation of code into
//! a running mixer, and the protocol around it.

mod audio_in;
mod clock;
mod controls;
mod events;
mod keyboard;
mod log;
mod midi_out;
mod project;
mod protocol;
mod quantize;
mod session;
mod sounding;

pub use audio_in::AudioInput;
pub use clock::ClockProbe;
pub use events::{At, Cancelled, Op, Sent};
pub use log::{Expression, LogEntry, LogInput, from_lines, to_lines};
pub use midi_out::MidiEvent;
pub use project::{Entry, Problem, Project};
pub use protocol::{
    ClientId, CompileTicket, CompiledTicket, Daemon, PROTOCOL_VERSION, Request, TOPICS,
};
pub use quantize::{Grid, QuantizeDefaults};
pub use session::{
    Accepted, Begin, ClockInfo, ControlSignal, EvalDone, EvalJob, Landed, ParsedEval, PendingInfo, Session,
    TrackStatus, Transport,
};

pub use session::realtime::{RealtimeConfig, RealtimeRenderer, RenderMetrics};
