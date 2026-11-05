//! The niminal live sequencer. Clips, scenes, patterns, quantized changes, live
//! commands and keyboards, played by sending timestamped events to the engine through its public API.
//!
//! A [`Live`] knows nothing of the engine's insides. It reads the engine's clock through an
//! [`InProcess`] connection, and a little ahead of it sends the notes, parameter changes, definitions that its project says are due.

pub mod clock;
pub mod input;
mod keyboard;
mod link;
mod live;
mod log;
mod offline;
mod protocol;
mod quantize;
mod service;
mod sync;

pub use link::InProcess;
pub use live::{
    Accepted, Begin, CancelDone, CancelJob, ClipStatus, ControlSignal, EvalDone, EvalJob, Landed,
    ArrangementStatus, Levels, Live, ParsedEval, PendingInfo, SceneStatus, Transport,
};
pub use log::{Expression, LiveInput, LiveLogEntry};

pub use offline::Offline;
pub use protocol::{CompileTicket, CompiledTicket, LiveServer, Request, TOPICS, Tap};
pub use quantize::QuantizeDefaults;
pub use sync::{ClockMessage, Realtime};
