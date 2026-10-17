//! The score model: plain data that every front-end produces and the engine
//! plays. Values keep their units as strings on the wire (`"-6db"`, `"1/4beat"`).

mod model;
mod value;

pub use model::{Event, ScoreError, Section, VERSION};
pub use value::{MAX_DEGREES, ParseError, Tempo, Time, Tuning, UnitError, Value};
