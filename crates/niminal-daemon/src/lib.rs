//! The niminal live session: quantized, transactional evaluation of code into
//! a running mixer, and the protocol around it.

mod log;
mod project;
mod protocol;
mod quantize;
mod session;

pub use log::{LogEntry, LogInput, from_lines, to_lines};
pub use protocol::{ClientId, Daemon, PROTOCOL_VERSION, TOPICS};
pub use project::{Entry, Problem, Project};
pub use quantize::{Grid, QuantizeDefaults};
pub use session::{Accepted, Landed, PendingInfo, Session, Transport};
