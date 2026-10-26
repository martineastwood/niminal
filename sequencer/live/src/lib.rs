//! The niminal live sequencer. Clips, scenes, patterns, quantized changes, live
//! commands and keyboards, played by sending timestamped events to the engine through its public API.
//!
//! A [`Live`] knows nothing of the engine's insides. It reads the engine's clock through an
//! [`EngineLink`], and a little ahead of it sends the notes, parameter changes, definitions that its project says are due.

mod keyboard;
mod link;
mod live;
mod log;
mod offline;
mod protocol;
mod quantize;
mod service;
mod ws_link;

pub use link::{EngineLink, InProcess};
pub use live::{
    Accepted, Begin, CancelDone, CancelJob, ClipStatus, ControlSignal, EvalDone, EvalJob, Landed,
    Live, ParsedEval, PendingInfo, SceneStatus, Transport,
};
pub use log::{Expression, LiveInput, LiveLogEntry};

pub use offline::Offline;
pub use protocol::{CompileTicket, CompiledTicket, LiveServer, MAIN_CHAIR, Request, TOPICS};
pub use quantize::QuantizeDefaults;
pub use ws_link::WsLink;
