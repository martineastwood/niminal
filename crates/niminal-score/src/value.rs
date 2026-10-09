use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A number with a unit. Strict units: converting to a different dimension is
/// an error, except where the spec allows it (note names to hz, db to gain).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    /// Unitless.
    Num(f64),
    Hz(f64),
    Db(f64),
    Seconds(f64),
    Beats(f64),
    Bars(f64),
    /// Semitones: a pitch interval, applied to a frequency or note.
    Semitones(f64),
    /// An angle in degrees; 0 is straight ahead and positive is to the right.
    Degrees(f64),
    /// A pitch as a MIDI note number; c4 is 60 and a4 is 440hz.
    Note(i32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, PartialEq)]
pub struct UnitError {
    pub expected: &'static str,
    pub got: Value,
}

impl fmt::Display for UnitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "expected {}, got `{}`", self.expected, self.got)
    }
}

impl std::error::Error for UnitError {}

const NOTE_LETTERS: [(char, i32); 7] = [('c', 0), ('d', 2), ('e', 4), ('f', 5), ('g', 7), ('a', 9), ('b', 11)];
const NOTE_NAMES: [&str; 12] = ["c", "c#", "d", "d#", "e", "f", "f#", "g", "g#", "a", "a#", "b"];

fn parse_note(s: &str) -> Option<i32> {
    let mut chars = s.chars();
    let letter = chars.next()?;
    let mut midi = NOTE_LETTERS.iter().find(|(l, _)| *l == letter)?.1;
    let rest = chars.as_str();
    let rest = match rest.chars().next() {
        Some('#') => {
            midi += 1;
            &rest[1..]
        }
        Some('b') => {
            midi -= 1;
            &rest[1..]
        }
        _ => rest,
    };
    let octave: i32 = rest.parse().ok()?;
    Some((octave + 1) * 12 + midi)
}

fn parse_number(s: &str) -> Option<f64> {
    match s.split_once('/') {
        Some((n, d)) => {
            let (n, d) = (n.parse::<f64>().ok()?, d.parse::<f64>().ok()?);
            (d != 0.0).then_some(n / d)
        }
        None => s.parse().ok(),
    }
}

impl FromStr for Value {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, ParseError> {
        let s = s.trim();
        if let Some(midi) = parse_note(s) {
            return Ok(Value::Note(midi));
        }

        let split = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
        let (number, unit) = s.split_at(split);
        let n = parse_number(number).ok_or_else(|| ParseError(format!("`{s}` is not a number with a unit")))?;
        Ok(match unit {
            "" => Value::Num(n),
            "hz" => Value::Hz(n),
            "khz" => Value::Hz(n * 1000.0),
            "db" => Value::Db(n),
            "sec" => Value::Seconds(n),
            "ms" => Value::Seconds(n / 1000.0),
            "beat" | "beats" => Value::Beats(n),
            "bar" | "bars" => Value::Bars(n),
            "st" => Value::Semitones(n),
            "deg" => Value::Degrees(n),
            _ => return Err(ParseError(format!("unknown unit `{unit}` in `{s}`"))),
        })
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Value::Num(n) => write!(f, "{n}"),
            Value::Hz(n) => write!(f, "{n}hz"),
            Value::Db(n) => write!(f, "{n}db"),
            Value::Seconds(n) => write!(f, "{n}sec"),
            Value::Beats(1.0) => write!(f, "1beat"),
            Value::Beats(n) => write!(f, "{n}beats"),
            Value::Bars(1.0) => write!(f, "1bar"),
            Value::Bars(n) => write!(f, "{n}bars"),
            Value::Semitones(n) => write!(f, "{n}st"),
            Value::Degrees(n) => write!(f, "{n}deg"),
            Value::Note(m) => write!(f, "{}{}", NOTE_NAMES[m.rem_euclid(12) as usize], m.div_euclid(12) - 1),
        }
    }
}

impl Value {
    fn mismatch<T>(self, expected: &'static str) -> Result<T, UnitError> {
        Err(UnitError { expected, got: self })
    }

    pub fn as_hz(self) -> Result<f64, UnitError> {
        match self {
            Value::Hz(h) => Ok(h),
            Value::Note(m) => Ok(440.0 * 2f64.powf((f64::from(m) - 69.0) / 12.0)),
            v => v.mismatch("a frequency (hz or a note name)"),
        }
    }

    /// Linear gain factor. Decibels convert; a bare number is taken as linear.
    pub fn as_gain(self) -> Result<f64, UnitError> {
        match self {
            Value::Db(d) => Ok(10f64.powf(d / 20.0)),
            Value::Num(n) => Ok(n),
            v => v.mismatch("a gain (db or a plain factor)"),
        }
    }

    /// An angle in degrees.
    pub fn as_angle(self) -> Result<f64, UnitError> {
        match self {
            Value::Degrees(d) => Ok(d),
            v => v.mismatch("an angle (deg)"),
        }
    }

    /// A pitch interval in semitones.
    pub fn as_semitones(self) -> Result<f64, UnitError> {
        match self {
            Value::Semitones(s) => Ok(s),
            v => v.mismatch("an interval (st)"),
        }
    }

    pub fn as_number(self) -> Result<f64, UnitError> {
        match self {
            Value::Num(n) => Ok(n),
            v => v.mismatch("a plain number"),
        }
    }

    pub fn as_time(self) -> Result<Time, UnitError> {
        match self {
            Value::Seconds(s) => Ok(Time::Seconds(s)),
            Value::Beats(b) => Ok(Time::Beats(b)),
            Value::Bars(b) => Ok(Time::Bars(b)),
            v => v.mismatch("a time (sec, ms, beats or bars)"),
        }
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// Accepts a unit string, or a bare JSON number as a unitless value.
impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Value;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a value string such as \"440hz\", or a number")
            }

            fn visit_str<E: de::Error>(self, s: &str) -> Result<Value, E> {
                s.parse().map_err(E::custom)
            }

            fn visit_f64<E: de::Error>(self, n: f64) -> Result<Value, E> {
                Ok(Value::Num(n))
            }

            fn visit_i64<E: de::Error>(self, n: i64) -> Result<Value, E> {
                Ok(Value::Num(n as f64))
            }

            fn visit_u64<E: de::Error>(self, n: u64) -> Result<Value, E> {
                Ok(Value::Num(n as f64))
            }
        }
        d.deserialize_any(V)
    }
}

/// A time in either musical or absolute terms. Both can appear in one score.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Time {
    Seconds(f64),
    Beats(f64),
    Bars(f64),
}

/// A constant tempo and meter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tempo {
    pub bpm: f64,
    pub beats_per_bar: f64,
}

impl Tempo {
    /// `bpm` in 4/4.
    pub fn new(bpm: f64) -> Self {
        Tempo { bpm, beats_per_bar: 4.0 }
    }
}

impl Time {
    pub fn to_seconds(self, tempo: Tempo) -> f64 {
        match self {
            Time::Seconds(s) => s,
            Time::Beats(b) => b * 60.0 / tempo.bpm,
            Time::Bars(b) => b * tempo.beats_per_bar * 60.0 / tempo.bpm,
        }
    }

    /// The length in beats, or `None` for an absolute time in seconds.
    pub fn to_beats(self, tempo: Tempo) -> Option<f64> {
        match self {
            Time::Seconds(_) => None,
            Time::Beats(b) => Some(b),
            Time::Bars(b) => Some(b * tempo.beats_per_bar),
        }
    }
}

impl From<Time> for Value {
    fn from(t: Time) -> Value {
        match t {
            Time::Seconds(s) => Value::Seconds(s),
            Time::Beats(b) => Value::Beats(b),
            Time::Bars(b) => Value::Bars(b),
        }
    }
}

impl FromStr for Time {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, ParseError> {
        let v: Value = s.parse()?;
        v.as_time().map_err(|e| ParseError(e.to_string()))
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Value::from(*self).fmt(f)
    }
}

impl Serialize for Time {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Time {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        v.as_time().map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Value {
        s.parse().unwrap()
    }

    #[test]
    fn parses_units() {
        assert_eq!(v("440hz"), Value::Hz(440.0));
        assert_eq!(v("1.2khz"), Value::Hz(1200.0));
        assert_eq!(v("-6db"), Value::Db(-6.0));
        assert_eq!(v("250ms"), Value::Seconds(0.25));
        assert_eq!(v("2sec"), Value::Seconds(2.0));
        assert_eq!(v("1/8beat"), Value::Beats(0.125));
        assert_eq!(v("4beats"), Value::Beats(4.0));
        assert_eq!(v("-45deg"), Value::Degrees(-45.0));
        assert_eq!(v("16bars"), Value::Bars(16.0));
        assert_eq!(v("1bar"), Value::Bars(1.0));
        assert_eq!(v("+7st"), Value::Semitones(7.0));
        assert_eq!(v("-12st"), Value::Semitones(-12.0));
        assert_eq!(v("0.5"), Value::Num(0.5));
        assert_eq!(v("-3"), Value::Num(-3.0));
    }

    #[test]
    fn parses_note_names() {
        assert_eq!(v("c4"), Value::Note(60));
        assert_eq!(v("a4"), Value::Note(69));
        assert_eq!(v("a3"), Value::Note(57));
        assert_eq!(v("c#4"), Value::Note(61));
        assert_eq!(v("bb2"), Value::Note(46));
        assert_eq!(v("e-1"), Value::Note(4));
    }

    #[test]
    fn rejects_bad_input() {
        for bad in ["", "hz", "440herz", "1/0beat", "x3", "4 bars"] {
            assert!(bad.parse::<Value>().is_err(), "{bad:?} should not parse");
        }
        assert_eq!("440herz".parse::<Value>().unwrap_err().0, "unknown unit `herz` in `440herz`");
    }

    #[test]
    fn display_round_trips() {
        for s in ["440hz", "-6db", "0.25sec", "1beat", "2.5beats", "0.5", "c4", "a#2", "e-1", "-45deg", "16bars", "1bar", "7st"] {
            assert_eq!(v(s).to_string(), s);
        }
    }

    #[test]
    fn conversions_follow_the_unit_rules() {
        assert!((v("a4").as_hz().unwrap() - 440.0).abs() < 1e-9);
        assert!((v("a3").as_hz().unwrap() - 220.0).abs() < 1e-9);
        assert!((v("-6db").as_gain().unwrap() - 0.501_187).abs() < 1e-6);
        assert_eq!(v("0.5").as_gain().unwrap(), 0.5);
        assert!(v("440").as_hz().is_err(), "bare numbers are not frequencies");
        assert!(v("-6db").as_hz().is_err());
        assert!(v("440hz").as_gain().is_err());
        assert_eq!(v("440hz").as_hz().unwrap(), 440.0);
        assert_eq!(v("-6db").as_hz().unwrap_err().to_string(), "expected a frequency (hz or a note name), got `-6db`");
    }

    #[test]
    fn beats_follow_tempo() {
        let t = Tempo::new(120.0);
        assert_eq!(Time::Beats(1.0).to_seconds(t), 0.5);
        assert_eq!(Time::Seconds(1.0).to_seconds(t), 1.0);
        assert_eq!(Time::Bars(2.0).to_seconds(t), 4.0, "two bars of 4/4 at 120bpm");
        let waltz = Tempo { beats_per_bar: 3.0, ..t };
        assert_eq!(Time::Bars(2.0).to_seconds(waltz), 3.0);
        assert_eq!(Time::Bars(2.0).to_beats(waltz), Some(6.0));
        assert_eq!(Time::Seconds(1.0).to_beats(t), None);
        assert!("440hz".parse::<Time>().is_err());
    }

    #[test]
    fn json_accepts_strings_and_numbers() {
        assert_eq!(serde_json::from_str::<Value>("\"-6db\"").unwrap(), Value::Db(-6.0));
        assert_eq!(serde_json::from_str::<Value>("0.5").unwrap(), Value::Num(0.5));
        assert_eq!(serde_json::from_str::<Value>("3").unwrap(), Value::Num(3.0));
        assert_eq!(serde_json::to_string(&Value::Hz(440.0)).unwrap(), "\"440hz\"");
        assert!(serde_json::from_str::<Time>("\"440hz\"").is_err());
    }
}
