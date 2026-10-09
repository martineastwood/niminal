/// The dimension of a value. Raw signals (oscillators, envelopes) are `Num`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Num,
    Hz,
    Db,
    Time,
}

impl Unit {
    pub fn describe(self) -> &'static str {
        match self {
            Unit::Num => "a plain number",
            Unit::Hz => "hz",
            Unit::Db => "db",
            Unit::Time => "a time",
        }
    }

    /// A suffix that turns a bare number into this unit, for "did you mean" hints.
    pub fn suffix(self) -> Option<&'static str> {
        match self {
            Unit::Num => None,
            Unit::Hz => Some("hz"),
            Unit::Db => Some("db"),
            Unit::Time => Some("sec"),
        }
    }
}
