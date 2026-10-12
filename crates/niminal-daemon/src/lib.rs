//! The niminal live session: quantized, transactional evaluation of code into
//! a running mixer, and the protocol around it.

mod audio_in;
mod controls;
mod keyboard;
mod log;
mod midi_out;
mod project;
mod protocol;
mod quantize;
mod session;
mod sounding;

pub use audio_in::AudioInput;
pub use midi_out::MidiEvent;
pub use log::{Expression, LogEntry, LogInput, from_lines, to_lines};
pub use protocol::{ClientId, CompiledTicket, Daemon, CompileTicket, PROTOCOL_VERSION, Request, TOPICS};
pub use project::{Entry, Problem, Project, expand_uses};
pub use quantize::{Grid, QuantizeDefaults};
pub use session::{Accepted, Begin, ControlSignal, EvalDone, EvalJob, Landed, PendingInfo, ParsedEval, Session, TrackStatus, Transport};

pub use session::realtime::{RealtimeConfig, RealtimeRenderer, RenderMetrics};
