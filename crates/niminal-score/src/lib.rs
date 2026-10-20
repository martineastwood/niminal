//! The values that clients and the engine share: quantities with units, and timed events. Values keep
//! their units as strings on the wire (`"-6db"`, `"1/4beat"`).

mod model;
mod value;

pub use model::Event;
pub use value::{MAX_DEGREES, ParseError, Tempo, Time, Tuning, UnitError, Value};
