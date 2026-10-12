/// The dimension of a value. Raw signals (oscillators, envelopes) are `Num`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Num,
    Hz,
    Db,
    Time,
    /// Degrees; 0 is straight ahead and positive is to the right.
    Angle,
    /// A pitch interval; applied to a frequency it scales it by a power of two.
    Semitones,
}

impl Unit {
    pub fn describe(self) -> &'static str {
        match self {
            Unit::Num => "a plain number",
            Unit::Hz => "hz",
            Unit::Db => "db",
            Unit::Time => "a time",
            Unit::Angle => "an angle",
            Unit::Semitones => "an interval",
        }
    }

    /// A suffix that turns a bare number into this unit, for "did you mean" hints.
    pub fn suffix(self) -> Option<&'static str> {
        match self {
            Unit::Num => None,
            Unit::Hz => Some("hz"),
            Unit::Db => Some("db"),
            Unit::Time => Some("sec"),
            Unit::Angle => Some("deg"),
            Unit::Semitones => Some("st"),
        }
    }
}

/// Result unit of `a * b`, or `None` if the units don't mix.
pub(crate) fn mul_unit(lu: Unit, ru: Unit) -> Option<Unit> {
    match (lu, ru) {
        (Unit::Num, Unit::Db) | (Unit::Db, Unit::Num) => Some(Unit::Num),
        (Unit::Db, _) | (_, Unit::Db) => None,
        (Unit::Num, u) | (u, Unit::Num) => Some(u),
        _ => None,
    }
}

/// Result unit of `a / b`, or `None` if the units don't mix.
pub(crate) fn div_unit(lu: Unit, ru: Unit) -> Option<Unit> {
    match (lu, ru) {
        (Unit::Db, _) | (_, Unit::Db) => None,
        (u, Unit::Num) => Some(u),
        (a, b) if a == b => Some(Unit::Num),
        _ => None,
    }
}

/// Result unit of `a % b`, or `None` if the units don't mix.
pub(crate) fn rem_unit(lu: Unit, ru: Unit) -> Option<Unit> {
    match (lu, ru) {
        (Unit::Db, _) | (_, Unit::Db) => None,
        (u, Unit::Num) => Some(u),
        (a, b) if a == b => Some(a),
        _ => None,
    }
}
