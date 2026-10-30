use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::cell::{Cell, RefCell};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Track {
    pub name: String,
    /// The track's effect chain, in signal order.
    #[serde(default)]
    pub fx: Vec<Device>,
    pub pan: f64,
    pub muted: bool,
    /// While any track is soloed, only soloed tracks sound.
    #[serde(default)]
    pub solo: bool,
    pub volume_db: f64,
    /// Where the strip's finished signal goes. A return can only feed a later return; the master has no output.
    #[serde(default)]
    pub output: Output,
    /// Knobs that each move several of this strip's effect parameters together.
    #[serde(default)]
    pub macros: Vec<Macro>,
}

/// One graphical language macro: `value` (0 to 1) expands to its linked parameter values.
/// The mixer and lanes both use the composition language's macro evaluator.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Macro {
    pub name: String,
    #[serde(default)]
    pub value: f64,
    #[serde(default)]
    pub links: Vec<MacroLink>,
}

/// A parameter a macro drives: at 0 the parameter sits at `min` of its range, at 1 at `max`, both
/// fractions of the range. `min` above `max` runs the parameter backwards.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MacroLink {
    pub device: String,
    pub parameter: String,
    pub min: f64,
    pub max: f64,
}

/// The destination of a track's signal: the master, one return track (making it a group), or nowhere,
/// leaving only what its send devices carry.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Output {
    #[default]
    Master,
    Return {
        index: usize,
    },
    None,
}

/// One effect in a track's chain. The `id` is stable across reordering, so lanes and
/// modulation can refer to the device and the engine node keeps its name.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Device {
    pub id: String,
    pub definition: String,
    #[serde(default)]
    pub bypass: bool,
    #[serde(default)]
    pub values: BTreeMap<String, f64>,
    #[serde(default)]
    pub modulations: BTreeMap<String, Lfo>,
    /// A send device: its output goes to this return track, and the chain carries on past it
    /// unchanged. Not on the master; a return's must be a later return.
    #[serde(default)]
    pub target: Option<usize>,
    /// A send taps the strip after its level and pan; `pre` taps it at its place in the chain instead.
    #[serde(default)]
    pub pre: bool,
    /// A pre-fader send cuts the signal after it: only sends carry the sound on.
    #[serde(default)]
    pub mute_source: bool,
}

impl Track {
    /// Settings out of range, or a malformed effect chain.
    pub fn invalid(&self) -> bool {
        !self.volume_db.is_finite()
            || !(-60.0..=6.0).contains(&self.volume_db)
            || !self.pan.is_finite()
            || !(-1.0..=1.0).contains(&self.pan)
            || self.fx.len() > MAX_DEVICES
            || matches!(self.output, Output::Return { index } if index >= MAX_RETURNS)
            || self.fx.iter().any(|d| d.target.is_some_and(|t| t >= MAX_RETURNS))
            || self.macros_invalid()
            || self.fx.iter().enumerate().any(|(i, device)| {
                device.id.is_empty()
                    || device.id.len() > 40
                    || !device
                        .id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
                    || self.fx[..i].iter().any(|other| other.id == device.id)
                    || device.values.values().any(|value| !value.is_finite())
                    || device.modulations.values().any(|lfo| !lfo.valid())
            })
    }

    /// Macros out of range, or two of them driving one parameter.
    fn macros_invalid(&self) -> bool {
        let fraction = |x: f64| (0.0..=1.0).contains(&x);
        let mut linked = BTreeSet::new();
        self.macros.len() > MAX_MACROS
            || self.macros.iter().any(|m| {
                m.name.len() > 40
                    || !fraction(m.value)
                    || m.links.len() > 64
                    || m.links.iter().any(|link| {
                        !fraction(link.min)
                            || !fraction(link.max)
                            || !linked.insert((link.device.as_str(), link.parameter.as_str()))
                    })
            })
    }

    /// The macro that drives this device parameter, with its link.
    pub fn macro_of(&self, device: &str, parameter: &str) -> Option<(usize, &MacroLink)> {
        self.macros.iter().enumerate().find_map(|(index, m)| {
            m.links.iter().find(|l| l.device == device && l.parameter == parameter).map(|l| (index, l))
        })
    }

    pub fn device(&self, id: &str) -> Option<&Device> {
        self.fx.iter().find(|device| device.id == id)
    }
    /// The devices that process audio: bypassed ones are left out of the engine graph.
    pub fn active(&self) -> impl Iterator<Item = &Device> {
        self.fx.iter().filter(|device| !device.bypass)
    }
}

pub const MAX_DEVICES: usize = 8;
pub const MAX_RETURNS: usize = 8;
/// Automation lanes a strip can have in a pattern.
pub const MAX_LANES: usize = 16;
pub const MAX_MACROS: usize = 15; // a macro is named by one hex digit in a `0yzz` command

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Lfo {
    pub enabled: bool,
    pub shape: String,
    pub period: f64,
    pub min: f64,
    pub max: f64,
}

impl Lfo {
    pub fn valid(&self) -> bool {
        ["sine", "triangle", "saw", "square"].contains(&self.shape.as_str())
            && self.period.is_finite()
            && (1.0..=256.0).contains(&self.period)
            && self.min.is_finite()
            && self.max.is_finite()
            && (0.0..=1.0).contains(&self.min)
            && (self.min..=1.0).contains(&self.max)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnvelopePoint {
    pub row: f64,
    pub value: f64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Envelope {
    pub enabled: bool,
    pub mode: String,
    pub points: Vec<EnvelopePoint>,
}
impl Envelope {
    pub fn valid(&self) -> bool {
        ["hold", "linear"].contains(&self.mode.as_str())
            && self.points.len() <= 4097
            && self.points.iter().enumerate().all(|(i, p)| {
                p.row.is_finite()
                    && (0.0..=256.0).contains(&p.row)
                    && p.value.is_finite()
                    && (i == 0 || p.row > self.points[i - 1].row)
            })
    }
    pub fn value(&self, row: f64, db: bool) -> Option<f64> {
        let before = self.points.iter().rev().find(|p| p.row <= row)?;
        let Some(after) = self.points.iter().find(|p| p.row > row) else {
            return Some(before.value);
        };
        if self.mode == "hold" {
            return Some(before.value);
        }
        let fraction = (row - before.row) / (after.row - before.row);
        Some(if db {
            20.0 * (10.0_f64.powf(before.value / 20.0) * (1.0 - fraction)
                + 10.0_f64.powf(after.value / 20.0) * fraction)
                .log10()
        } else {
            before.value + (after.value - before.value) * fraction
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FxLane {
    pub target: Option<crate::commands::FxTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<Envelope>,
}
impl FxLane {
    #[cfg(test)]
    pub fn empty() -> Self {
        Self {
            target: Some(crate::commands::FxTarget::Volume),
            envelope: None,
        }
    }
    /// A lane that sets each of `values` at its row and holds it until the next.
    #[cfg(test)]
    pub fn held(&mut self, values: &[(usize, f64)]) {
        self.envelope = Some(Envelope {
            enabled: true,
            mode: "hold".into(),
            points: values.iter().map(|&(row, value)| EnvelopePoint { row: row as f64, value }).collect(),
        });
    }
}

/// A command on a track, a return or the master, as in Renoise's track commands. The cell is written `Lxx`, `Pxx`
/// or four hex digits `xyzz`, with place `0` for a macro:
///
/// | | |
/// |-|-|
/// | `Lxx` | the strip's volume: `00` is silent, `FF` is +3 dB |
/// | `Pxx` | the strip's pan: `00` is left, `80` centre, `FF` right |
/// | `xyzz` | the effect in place `x` of the strip's chain (from 1) gets its parameter `y` (from 1) set to `zz`, from its least at `00` to its most at `FF` |
/// | `0yzz` | the strip's macro `y` (from 1) is set to `zz`, from 0% at `00` to 100% at `FF` |
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrackCommand {
    Volume(u8),
    Pan(u8),
    Device { device: u8, parameter: u8, value: u8 },
    Macro { index: u8, value: u8 },
}

impl TrackCommand {
    pub fn parse(text: &str) -> Option<Self> {
        let hex = |digits: &str| digits.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)).then(|| u8::from_str_radix(digits, 16).ok()).flatten();
        match text.as_bytes() {
            [b'L', ..] if text.len() == 3 => hex(&text[1..]).map(Self::Volume),
            [b'P', ..] if text.len() == 3 => hex(&text[1..]).map(Self::Pan),
            _ if text.len() == 4 && text.is_ascii() => {
                let (device, parameter) = (hex(&text[..1])?, hex(&text[1..2])?);
                let value = hex(&text[2..])?;
                if device == 0 { return (1..=MAX_MACROS as u8).contains(&parameter).then_some(Self::Macro { index: parameter, value }); }
                (parameter > 0).then_some(Self::Device { device, parameter, value })
            }
            _ => None,
        }
    }

    /// What the strip is told to do: the target and the value in the units its lane would hold.
    pub fn target(self, strip: &Track, parameters: &crate::commands::EffectParameters) -> Option<(crate::commands::FxTarget, f64)> {
        use crate::commands::FxTarget;
        match self {
            Self::Volume(value) => Some((FxTarget::Volume, if value == 0 { -60.0 } else { (20.0 * (f64::from(value) / 255.0).log10() + 3.0).clamp(-60.0, 6.0) })),
            Self::Pan(value) => Some((FxTarget::Pan, ((f64::from(value) - 128.0) / 127.0).clamp(-1.0, 1.0))),
            Self::Macro { index, value } => Some((FxTarget::Macro { index: usize::from(index) - 1 }, f64::from(value) / 255.0)),
            Self::Device { device, parameter, value } => {
                let device = strip.fx.get(usize::from(device) - 1)?;
                let info = parameters.get(&device.definition)?.get(usize::from(parameter) - 1)?;
                Some((FxTarget::Effect { device: device.id.clone(), parameter: info.name.clone() }, info.min + f64::from(value) / 255.0 * (info.max - info.min)))
            }
        }
    }
}

pub const MAX_NOTE_COLUMNS: usize = 8;
pub const MAX_TRACK_COMMAND_COLUMNS: usize = 4;

/// One note column of a track. Every sub-column holds a cell per row; the optional ones
/// may be left empty, which reads as blank everywhere.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct NoteColumn {
    /// A note such as `C-4`, `OFF` to let the note that is sounding release, or `CUT` to stop it at once.
    pub notes: Vec<Option<String>>,
    /// The instrument that plays this row's note and the notes below it, by definition id.
    #[serde(default)]
    pub inst: Vec<Option<String>>,
    /// Level from 0 (silent) to 128 (full).
    #[serde(default)]
    pub vol: Vec<Option<String>>,
    /// Position from 0 (left) through 64 (centre) to 128 (right).
    #[serde(default)]
    pub pan: Vec<Option<String>>,
    /// How late the note starts, in 256ths of a row.
    #[serde(default)]
    pub delay: Vec<Option<String>>,
    /// Command columns: a letter and two hex digits in each cell, such as `S20` or `R41`: see [`Command`].
    /// Each is a column of cells for every row; a note column has none until it is given some.
    #[serde(default)]
    pub fx: Vec<Vec<Option<String>>>,
}

pub const MAX_FX_COLUMNS: usize = 4;
/// Ticks in a row, as in Renoise: the commands that count ticks and per-tick speeds use it.
const TICKS: f64 = 12.0;

/// A command cell: a letter and two hex digits, `S20`. These follow Renoise's effect commands where
/// the engine has the same thing:
///
/// | | |
/// |-|-|
/// | `Sxx` | start the sample `xx`/256 of the way in |
/// | `B00`, `B01` | play the sample backwards, forwards |
/// | `Gxx` | glide the held note to this one at `xx` sixteenths of a semitone per tick (`FF` at once); `G00` repeats the last `G` above it |
/// | `Uxx`, `Dxx` | slide the held note up or down by `xx` sixteenths of a semitone per tick during the row; `00` repeats the last one |
/// | `Rxy` | retrigger every `y` ticks (1 to 6), each hit changed in level by `x`'s factor |
/// | `C0y` | cut the note `y` ticks (1 to 11) into the row |
/// | `Yxx` | play the note with probability `xx`/255 |
/// | `Ixx`, `Oxx` | fade the held note's level in or out by `xx` volume units (0 to 128) during the row; `00` repeats the last one |
///
/// A cell may instead hold a [`TrackCommand`] (`Lxx`, `Pxx` or `xyzz`), which acts on the whole track and which the note accessors skip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Command {
    pub letter: char,
    pub value: u8,
}

impl Command {
    pub fn parse(text: &str) -> Option<Self> {
        let mut chars = text.chars();
        let letter = chars.next().filter(char::is_ascii_uppercase)?;
        let digits = chars.as_str();
        if digits.len() != 2 || !digits.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)) {
            return None;
        }
        Some(Self { letter, value: u8::from_str_radix(digits, 16).ok()? })
    }
    fn x(self) -> u8 {
        self.value >> 4
    }
    fn y(self) -> u8 {
        self.value & 15
    }
    /// Why the engine cannot play this command, if it cannot.
    pub fn problem(self) -> Option<&'static str> {
        match self.letter {
            'S' => None,
            'B' => (self.value > 1).then_some("B is B00 (backwards) or B01 (forwards)"),
            'G' | 'U' | 'D' => None,
            'Y' => (self.value == 0).then_some("Y runs from 01 to FF"),
            'I' | 'O' => (self.value > 0x80).then_some("I and O run from 00 to 80"),
            'R' => (self.y() == 0 || self.y() > 6).then_some("R is Rxy with y from 1 to 6 ticks"),
            'C' => (self.x() != 0 || self.y() == 0 || self.y() > 11).then_some("C is C0y with y from 1 to 11 ticks"),
            _ => Some("Commands are S, B, G, U, D, R, C, Y, I and O for a note, and L, P or four hex digits for the track"),
        }
    }
}

/// Where a new note's sample starts and which way it plays.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Start {
    /// The share of the sample to skip, from 0 up to 0.99.
    pub offset: Option<f64>,
    pub reverse: bool,
}

/// What a pitch command asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pitch {
    /// `Gxx`: glide the note that is held in this column to this one, at this speed in sixteenths of a semitone per tick.
    Glide(f64),
    /// `Uxx`, `Dxx`: slide the held note up or down by this many semitones during this row.
    Slide(f64),
}

impl Pitch {
    /// How many rows a glide of `speed` takes to cover `semitones`; a speed of 255 is at once.
    pub fn glide_rows(speed: f64, semitones: f64) -> f64 {
        if speed >= 255.0 { 0.0 } else { semitones.abs() * 16.0 / (speed * TICKS) }
    }
}

/// A retrigger: `R41` plays the note again every tick, each hit changed in level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Retrigger {
    /// How many times the note plays, from 2 to 12.
    pub hits: u32,
    /// How each hit's level differs from the one before, from -1 to 1.
    pub change: f64,
}

impl Retrigger {
    fn from(command: Command) -> Self {
        let change = match command.x() {
            1..=5 => -[0.03, 0.06, 0.12, 0.25, 0.5][usize::from(command.x()) - 1],
            6 => -0.33,
            7 => -0.5,
            9..=13 => [0.03, 0.06, 0.12, 0.25, 0.5][usize::from(command.x()) - 9],
            14 => 0.5,
            15 => 1.0,
            _ => 0.0,
        };
        Self { hits: (TICKS / f64::from(command.y())) as u32, change }
    }
    /// How loud hit `hit` (counting from 0) is against the note's own level. Each hit is the one before
    /// times `1 + change`; when that grows, the last hit is the note's level and the earlier ones are quieter.
    pub fn gain(&self, hit: u32) -> f64 {
        let ratio = 1.0 + self.change;
        let steps = if self.change > 0.0 { hit as i32 - (self.hits as i32 - 1) } else { hit as i32 };
        ratio.powi(steps)
    }
}

/// Where the sample starts, from the commands of a row.
pub fn start_of(commands: &[Command]) -> Start {
    let mut start = Start::default();
    for command in commands {
        match command.letter {
            'S' => start.offset = Some((f64::from(command.value) / 256.0).min(0.99)),
            'B' => start.reverse = command.value == 0,
            _ => {}
        }
    }
    start
}

/// The retrigger, from the commands of a row.
pub fn retrigger_of(commands: &[Command]) -> Option<Retrigger> {
    commands.iter().rev().find(|c| c.letter == 'R').map(|c| Retrigger::from(*c))
}

/// Parses the commands of a row, ignoring any that are not valid.
pub fn commands_of<'a>(cells: impl IntoIterator<Item = &'a str>) -> Vec<Command> {
    cells.into_iter().filter_map(Command::parse).filter(|c| c.problem().is_none()).collect()
}

#[cfg(test)]
impl NoteColumn {
    pub fn blank(rows: usize) -> Self {
        Self {
            notes: vec![None; rows],
            ..Self::default()
        }
    }
}

impl NoteColumn {
    fn validate(&self, rows: usize) -> Result<(), String> {
        let sized =
            |len: usize, optional: bool| len <= 256 && (len >= rows || optional && len == 0);
        if !sized(self.notes.len(), false)
            || !sized(self.inst.len(), true)
            || !sized(self.vol.len(), true)
            || !sized(self.pan.len(), true)
            || !sized(self.delay.len(), true)
            || self.fx.len() > MAX_FX_COLUMNS
            || self.fx.iter().any(|cells| !sized(cells.len(), false))
        {
            return Err("Every note sub-column needs a cell for each row".into());
        }
        for note in self.notes.iter().flatten() {
            if note != "OFF" && note != "CUT" {
                pitch(note)?;
            }
        }
        let within = |values: &[Option<String>], max: u32| {
            values
                .iter()
                .flatten()
                .all(|v| v.parse::<u32>().is_ok_and(|n| n <= max))
        };
        if !within(&self.vol, 128) || !within(&self.pan, 128) || !within(&self.delay, 255) {
            return Err("Volume and pan run 0–128 and delay 0–255".into());
        }
        for command in self.fx.iter().flatten().flatten() {
            if TrackCommand::parse(command).is_some() {
                continue;
            }
            let Some(parsed) = Command::parse(command) else {
                return Err(format!("{command} is not a command: a letter and two hex digits such as S20, or four hex digits for an effect"));
            };
            if let Some(problem) = parsed.problem() {
                return Err(problem.into());
            }
        }
        if (0..rows).any(|row| {
            let commands = self.commands(row);
            commands.iter().any(|c| c.letter == 'G') && retrigger_of(&commands).is_some()
        }) {
            return Err("A glide cannot share a row with a retrigger".into());
        }
        Ok(())
    }
    fn text(values: &[Option<String>], row: usize) -> Option<&str> {
        values.get(row).and_then(|v| v.as_deref())
    }
    pub fn note(&self, row: usize) -> Option<&str> {
        Self::text(&self.notes, row)
    }
    /// The instrument for a note at `row`: the one set on it, or the last one set above it.
    pub fn instrument(&self, row: usize) -> Option<&str> {
        (0..=row).rev().find_map(|at| Self::text(&self.inst, at))
    }
    pub fn volume(&self, row: usize) -> Option<u32> {
        Self::text(&self.vol, row).and_then(|v| v.parse().ok())
    }
    pub fn panning(&self, row: usize) -> Option<u32> {
        Self::text(&self.pan, row).and_then(|v| v.parse().ok())
    }
    /// The commands in a row, from every command column.
    pub fn commands(&self, row: usize) -> Vec<Command> {
        let resolved = self.fx.iter().filter_map(|cells| {
            let command = Command::parse(Self::text(cells, row)?)?;
            if command.value != 0 || !"GUDIO".contains(command.letter) {
                return Some(command);
            }
            // 00 repeats the last value of the same command above it in the column; with none, it does nothing
            cells[..row].iter().rev().flatten().filter_map(|text| Command::parse(text)).find(|c| c.letter == command.letter && c.value != 0)
        });
        resolved.filter(|c| c.problem().is_none()).collect()
    }
    pub fn start(&self, row: usize) -> Start {
        start_of(&self.commands(row))
    }
    pub fn pitch(&self, row: usize) -> Option<Pitch> {
        self.commands(row).iter().rev().find_map(|c| match c.letter {
            'G' => Some(Pitch::Glide(f64::from(c.value))),
            'U' => Some(Pitch::Slide(f64::from(c.value) * TICKS / 16.0)),
            'D' => Some(Pitch::Slide(-f64::from(c.value) * TICKS / 16.0)),
            _ => None,
        })
    }
    pub fn retrigger(&self, row: usize) -> Option<Retrigger> {
        retrigger_of(&self.commands(row))
    }
    /// The chance, as a percent, that the note in this row plays.
    pub fn chance(&self, row: usize) -> u32 {
        self.commands(row).iter().rev().find(|c| c.letter == 'Y').map_or(100, |c| (f64::from(c.value) * 100.0 / 255.0).round() as u32)
    }
    /// How far the level of the note held in this column moves during this row, in the volume cells' units.
    pub fn fade(&self, row: usize) -> Option<f64> {
        self.commands(row).iter().rev().find_map(|c| match c.letter {
            'I' => Some(f64::from(c.value)),
            'O' => Some(-f64::from(c.value)),
            _ => None,
        })
    }
    /// The most of a row the note in this row may sound for, if it is cut.
    pub fn length(&self, row: usize) -> Option<f64> {
        self.commands(row).iter().rev().find(|c| c.letter == 'C').map(|c| f64::from(c.y()) / TICKS)
    }
    /// The delay as a fraction of a row, from 0 up to just under 1.
    pub fn delayed(&self, row: usize) -> f64 {
        f64::from(
            Self::text(&self.delay, row)
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(0),
        ) / 256.0
    }
}

/// BPM is quarter notes per minute; LPB is rows per quarter note. Meter only groups rows.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Timing {
    pub bpm: f64,
    pub rows_per_beat: u32,
    pub beats_per_bar: u32,
    pub beat_unit: u32,
    /// Linear BPM automation, in pattern rows. Before the first point, use `bpm`.
    pub tempo: Vec<EnvelopePoint>,
}
impl Timing {
    fn valid(&self, rows: usize) -> bool {
        self.bpm.is_finite() && (20.0..=300.0).contains(&self.bpm)
            && (1..=16).contains(&self.rows_per_beat)
            && (1..=32).contains(&self.beats_per_bar)
            && [1, 2, 4, 8, 16].contains(&self.beat_unit)
            && self.tempo.len() <= 257
            && self.tempo.iter().enumerate().all(|(i, p)| p.row.is_finite()
                && (0.0..=rows as f64).contains(&p.row) && p.value.is_finite()
                && (20.0..=300.0).contains(&p.value)
                && (i == 0 || p.row > self.tempo[i-1].row))
    }
    fn bpm_at(&self, row: f64) -> f64 {
        let Some(before) = self.tempo.iter().rev().find(|p| p.row <= row) else { return self.bpm; };
        let Some(after) = self.tempo.iter().find(|p| p.row > row) else { return before.value; };
        before.value + (after.value - before.value) * (row - before.row) / (after.row - before.row)
    }
    fn seconds(&self, start: f64, end: f64) -> f64 {
        let mut at = start;
        let mut seconds = 0.0;
        while at < end {
            let next = self.tempo.iter().find(|p| p.row > at).map_or(end, |p| p.row.min(end));
            let a = self.bpm_at(at);
            // A first point after `at` is a step from the base BPM, not a ramp into it.
            let b = if self.tempo.first().is_some_and(|p| p.row > at) { a } else { self.bpm_at(next) };
            let area = if (b-a).abs() < 1e-9 { (next-at)/a } else { (next-at) * (b/a).ln() / (b-a) };
            seconds += 60.0 / f64::from(self.rows_per_beat) * area;
            at = next;
        }
        seconds
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Pattern {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<Timing>,
    /// The swing for this pattern, 0 (straight) to 75 percent, instead of the song's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swing: Option<f64>,
    pub id: String,
    pub name: String,
    pub rows: usize,
    /// Note columns, by track.
    pub notes: Vec<Vec<NoteColumn>>,
    /// Parameter lanes, by track.
    pub fx: Vec<Vec<FxLane>>,
    /// Parameter lanes of each return, in order. Returns and the master have no notes.
    pub return_fx: Vec<Vec<FxLane>>,
    pub master_fx: Vec<FxLane>,
    /// Track commands of the returns, then the master, which have no note columns: each has up to four
    /// columns of cells (a track's are in the fx columns of its note columns): see [`TrackCommand`].
    #[serde(default)]
    pub commands: Vec<Vec<Vec<Option<String>>>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Song {
    pub bpm: f64,
    pub rows_per_beat: u32,
    /// How much of every pair of rows' time moves to the first row: 0 (straight) to 75 percent.
    #[serde(default)]
    pub swing: f64,
    pub tracks: Vec<Track>,
    /// The final stage every track feeds: a level, a pan and an effect chain.
    #[serde(default)]
    pub master: Track,
    /// Auxiliary tracks that tracks send to: each has an effect chain and feeds the master.
    #[serde(default)]
    pub returns: Vec<Track>,
    pub patterns: Vec<Pattern>,
    pub sequence: Vec<String>,
    /// Song playback repeats these sequence positions (inclusive) once it reaches them, until stopped.
    #[serde(default)]
    pub loop_range: Option<LoopRange>,
    /// By sequence step, then track: where the track plays from if not the step's own pattern.
    /// Steps and tracks beyond the ends of these lists play their own pattern.
    #[serde(default)]
    pub matrix: Vec<Vec<Option<TrackSource>>>,
    /// Each instrument's parameter values, by definition id; notes start with these.
    #[serde(default)]
    pub instrument_values: BTreeMap<String, BTreeMap<String, f64>>,
}

/// Where one track plays from in one sequence step, when not from the step's own pattern.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TrackSource {
    /// The track is silent for the whole step, and a note still sounding from before ends there.
    Mute,
    /// The track plays another pattern's notes and lanes. If that pattern is shorter than the step,
    /// the track falls silent after its last row.
    Pattern { id: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub struct LoopRange {
    pub start: usize,
    pub end: usize,
}

impl Song {
    pub fn validate(&self) -> Result<(), String> {
        if !self.bpm.is_finite()
            || !(20.0..=300.0).contains(&self.bpm)
            || !(1..=16).contains(&self.rows_per_beat)
            || !(1..=16).contains(&self.tracks.len())
            || self.tracks.iter().any(Track::invalid)
            || self.master.invalid()
            || self.returns.len() > MAX_RETURNS
            || self.returns.iter().any(Track::invalid)
            || self.master.output != Output::Master
            || self.master.fx.iter().any(|d| d.target.is_some())
            || self.returns.iter().enumerate().any(|(i, t)| {
                matches!(t.output, Output::Return { index } if index <= i)
                    || t.fx.iter().any(|d| d.target.is_some_and(|target| target <= i))
            })
            || self
                .instrument_values
                .values()
                .flatten()
                .any(|(_, value)| !value.is_finite())
            || self.patterns.is_empty()
            || self.patterns.len() > 128
            || self.sequence.is_empty()
            || self.sequence.len() > 1024
        {
            return Err("Invalid song size, tempo or track settings".into());
        }
        let mut ids = BTreeSet::new();
        for pattern in &self.patterns {
            if pattern.id.is_empty()
                || !ids.insert(&pattern.id)
                || !(1..=256).contains(&pattern.rows)
                || pattern.timing.as_ref().is_some_and(|t| !t.valid(pattern.rows))
                || pattern.notes.len() != self.tracks.len()
                || pattern
                    .notes
                    .iter()
                    .any(|columns| columns.is_empty() || columns.len() > MAX_NOTE_COLUMNS)
            {
                return Err(
                    "Patterns need unique IDs, 1–256 rows and 1–8 note columns per track".into(),
                );
            }
            for column in pattern.notes.iter().flatten() {
                column.validate(pattern.rows)?;
            }
            if !pattern.commands.is_empty() {
                let columns = self.returns.len() + 1;
                if pattern.commands.len() != columns
                    || pattern.commands.iter().any(|strip| strip.len() > MAX_TRACK_COMMAND_COLUMNS || strip.iter().any(|cells| cells.len() < pattern.rows || cells.len() > 256))
                {
                    return Err("Patterns need up to four track command columns, with a cell for each row, for every return and the master".into());
                }
                if pattern.commands.iter().flatten().flatten().flatten().any(|text| TrackCommand::parse(text).is_none()) {
                    return Err("A track command is Lxx, Pxx or four hex digits for a device parameter, such as 12FF".into());
                }
            }
            if pattern.fx.len() != self.tracks.len() || pattern.return_fx.len() != self.returns.len() {
                return Err("Patterns need parameter lanes for every track and return, and the master".into());
            }
            let groups = || pattern.fx.iter().chain(&pattern.return_fx).chain([&pattern.master_fx]);
            if groups().any(|lanes| {
                lanes.is_empty() || lanes.len() > MAX_LANES
            }) {
                return Err("Patterns need 1–16 parameter lanes per track, return and the master".into());
            }
            if pattern.return_fx.iter().flatten().chain(&pattern.master_fx).any(|lane| matches!(lane.target, Some(crate::commands::FxTarget::Instrument { .. }))) {
                return Err("Returns and the master have no instruments to automate".into());
            }
            for lane in groups().flatten() {
                if lane.envelope.as_ref().is_some_and(|e| !e.valid()) {
                    return Err("Invalid automation envelope".into());
                }
                if lane.target.is_none() && lane.envelope.as_ref().is_some_and(|e| !e.points.is_empty()) {
                    return Err("Choose a parameter for every populated lane".into());
                }
            }
        }
        if self.sequence.iter().any(|id| !ids.contains(id)) {
            return Err("Sequence references a missing pattern".into());
        }
        if self.matrix.len() > self.sequence.len()
            || self.matrix.iter().any(|step| {
                step.len() > self.tracks.len()
                    || step.iter().flatten().any(|source| {
                        matches!(source, TrackSource::Pattern { id } if !ids.contains(id))
                    })
            })
        {
            return Err("The matrix must name existing patterns and tracks".into());
        }
        let swing = |value: f64| value.is_finite() && (0.0..=75.0).contains(&value);
        if !swing(self.swing) || self.patterns.iter().any(|p| p.swing.is_some_and(|value| !swing(value))) {
            return Err("Swing runs from 0 to 75".into());
        }
        if self
            .loop_range
            .is_some_and(|l| l.start > l.end || l.end >= self.sequence.len())
        {
            return Err("The loop must lie within the sequence".into());
        }
        Ok(())
    }

    pub fn row_samples(&self, sample_rate: f64) -> f64 {
        sample_rate * 60.0 / self.bpm / f64::from(self.rows_per_beat)
    }

    pub fn same_timeline(&self, other: &Self) -> bool {
        self.tracks.len() == other.tracks.len()
            && self.sequence == other.sequence
            && self.patterns.iter().all(|p| {
                other
                    .patterns
                    .iter()
                    .any(|q| p.id == q.id && p.rows == q.rows)
            })
    }
}

/// A note that an instrument-parameter lane can still change: it has an id, and sounds until `until`.
struct Sounding {
    id: u64,
    begins: u64,
    track: usize,
    instrument: String,
    until: u64,
}

/// A change of pitch to a held note: from `delay` rows into row `index`, over `rows` rows, to `midi`.
struct Bend {
    index: u64,
    delay: f64,
    rows: f64,
    midi: f64,
}

/// A change of level to a held note: from `delay` rows into row `index`, over one row, to `volume` (0 to 128).
struct Change {
    index: u64,
    delay: f64,
    volume: f64,
}

/// How long a note is held, in rows from its own row, how it ends, and what happens to its pitch meanwhile.
struct Hold {
    rows: f64,
    cut: bool,
    bends: Vec<Bend>,
    changes: Vec<Change>,
    /// The glide notes whose sound this note carries on.
    absorbed: Vec<(u64, usize, usize)>,
}

/// The frequency of a MIDI note number.
fn hertz(midi: f64) -> f64 {
    440.0 * ((midi - 69.0) / 12.0).exp2()
}

/// Names the notes that a cut has to find again; shared so a new playback never reuses one still sounding.
pub static NOTE_IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// No note is shorter than this many rows, so back-to-back notes with delays still sound.
const MIN_HOLD: f64 = 0.01;

/// A note's volume cell (0–128, 128 being full level) as decibels.
pub fn volume_db(volume: u32) -> f64 {
    20.0 * (f64::from(volume.min(128)) / 128.0).max(1e-6).log10()
}

/// A note's pan cell (0–128, 64 being centre) as degrees; 30° is a hard pan in stereo.
pub fn pan_degrees(pan: u32) -> f64 {
    (f64::from(pan.min(128)) - 64.0) / 64.0 * 30.0
}

pub fn pitch(note: &str) -> Result<String, String> {
    let bytes = note.as_bytes();
    if bytes.len() != 3
        || !(b'A'..=b'G').contains(&bytes[0])
        || !b"-#".contains(&bytes[1])
        || !(b'0'..=b'8').contains(&bytes[2])
    {
        return Err(format!("Invalid tracker note: {note}"));
    }
    Ok(note.replace('-', "").to_lowercase())
}

/// A note's place on the keyboard as a MIDI number, with middle C (`C-4`) at 60.
fn midi(note: &str) -> Option<f64> {
    let bytes = note.as_bytes();
    let (letter, mark, octave) = (*bytes.first()?, *bytes.get(1)?, *bytes.get(2)?);
    let base = match letter {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return None,
    };
    let octave = i32::from(octave.checked_sub(b'0')?);
    Some(f64::from((octave + 1) * 12 + base + i32::from(mark == b'#')))
}

/// Sets what a note's start asks of its arguments, when the instrument has the parameter it needs
/// (`begin` for an offset, `rate` for playing backwards).
pub fn start_args(args: &mut Value, start: Start, parameters: &[crate::commands::Parameter]) {
    let has = |name: &str| parameters.iter().any(|p| p.name == name);
    if let Some(share) = start.offset.filter(|_| has("begin")) {
        args["begin"] = json!(share);
    }
    if start.reverse && has("rate") {
        args["rate"] = json!(-1.0);
    }
}

/// A note's arguments with its level scaled by `gain` (1 leaves them as they are), for one hit of a retrigger.
/// The level it starts from is the note's own `gain`, or else the instrument's default for one.
pub fn hit_args(args: &Value, gain: f64, parameters: &[crate::commands::Parameter]) -> Value {
    let mut args = args.clone();
    if (gain - 1.0).abs() < 1e-9 {
        return args;
    }
    let db = |linear: f64| 20.0 * linear.max(1e-6).log10();
    let base = match args.get("gain") {
        Some(Value::String(text)) => text.strip_suffix("db").and_then(|n| n.parse().ok()),
        Some(value) => value.as_f64().map(db),
        None => parameters.iter().find(|p| p.name == "gain").map(|p| if p.unit.as_deref() == Some("db") { p.default } else { db(p.default) }),
    };
    args["gain"] = json!(format!("{:.3}db", base.unwrap_or(0.0) + db(gain)));
    args
}

/// A sampler with a `from_bpm` plays in time with the song: it is stretched by the tempo its sample was
/// recorded at over the tempo `bpm` now, on top of its own `stretch`, and so keeps its pitch. With a `fit_mode`
/// of 1 (repitch) it plays faster or slower by that same ratio instead, on top of its own `rate`, as tape would,
/// and the pitch follows.
pub fn fit_to_tempo(args: &mut Value, parameters: &[crate::commands::Parameter], bpm: f64) {
    let value = |name: &str| args.get(name).and_then(Value::as_f64).or_else(|| parameters.iter().find(|p| p.name == name).map(|p| p.default));
    let Some(from) = value("from_bpm").filter(|from| *from > 0.0) else {
        return;
    };
    if value("fit_mode").is_some_and(|mode| mode >= 0.5) {
        if parameters.iter().any(|p| p.name == "rate") {
            args["rate"] = json!((value("rate").unwrap_or(1.0) * bpm / from).clamp(-4.0, 4.0));
        }
    } else if parameters.iter().any(|p| p.name == "stretch") {
        args["stretch"] = json!((value("stretch").unwrap_or(1.0) * from / bpm).clamp(0.25, 4.0));
    }
}

/// The arguments that make an instrument play `note`: its pitch, or for a sliced sample (one with
/// `slices` pieces) the piece at that place on the keyboard counting up from `C-4`, at its recorded
/// pitch. None when the note has no piece.
pub fn note_args(note: &str, slices: Option<usize>) -> Result<Option<Value>, String> {
    let name = pitch(note)?;
    let Some(count) = slices else {
        return Ok(Some(json!({"freq": name})));
    };
    let piece = midi(note).ok_or_else(|| format!("Invalid tracker note: {note}"))? - 60.0;
    Ok((0.0..count as f64)
        .contains(&piece)
        .then(|| json!({"freq": "c4", "sample": piece as u32})))
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Pattern,
    Song,
}

pub struct Playback {
    seconds: RefCell<Vec<f64>>,
    pub origin: u64,
    pub next_row: u64,
    pub parameters: crate::commands::EffectParameters,
    pub instrument_parameters: crate::commands::EffectParameters,
    /// How many pieces each sliced sample instrument has, by instrument id.
    pub instrument_slices: BTreeMap<String, usize>,
    /// Notes still sounding that instrument-parameter lanes change as they pass.
    sounding: Vec<Sounding>,
    /// Notes (row, track, column) that a glide has taken over, so they start no note of their own.
    absorbed: BTreeSet<(u64, usize, usize)>,
    /// The dice rolled for notes that might not play, by row, track and column, kept until the row has
    /// played so a note's hold, which looks ahead, and the note itself agree. Each roll is a number
    /// from 0 up to 1, and the note plays when it falls under the chance.
    rolls: RefCell<BTreeMap<(u64, usize, usize), f64>>,
    random: Cell<u64>,
    /// Each track's instrument parameter lanes, by instrument and parameter, as of the last row played.
    lane_state: Vec<BTreeMap<(String, String), f64>>,
    /// The tempo and meter (beats in a bar) the engine was last told, so it hears of a change and no more.
    told_tempo: Cell<Option<(f64, f64)>>,
    pub song: Song,
    pub mode: Mode,
    pub pattern_id: String,
    pub start_position: usize,
}

pub struct Position<'a> {
    pub pattern: &'a Pattern,
    pub row: usize,
    pub sequence_position: Option<usize>,
}

impl Playback {
    pub fn new(
        origin: u64,
        song: Song,
        mode: Mode,
        pattern_id: String,
        start_position: usize,
    ) -> Result<Self, String> {
        song.validate()?;
        if !song.patterns.iter().any(|p| p.id == pattern_id)
            || start_position >= song.sequence.len()
        {
            return Err("Select an existing pattern and sequence position".into());
        }
        let play = Self {
            seconds: RefCell::new(vec![0.0]),
            origin,
            next_row: 0,
            parameters: BTreeMap::new(),
            instrument_parameters: BTreeMap::new(),
            instrument_slices: BTreeMap::new(),
            absorbed: Default::default(),
            rolls: RefCell::new(BTreeMap::new()),
            random: Cell::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0x9E37_79B9_7F4A_7C15, |d| d.as_nanos() as u64),
            ),
            sounding: Vec::new(),
            lane_state: Vec::new(),
            told_tempo: Cell::new(None),
            song,
            mode,
            pattern_id,
            start_position,
        };
        // the start of playing says this tempo, so only a change from it is told
        play.told_tempo.set(Some(play.start_tempo()));
        Ok(play)
    }

    /// The tempo and meter (beats in a bar) the first row plays at.
    pub fn start_tempo(&self) -> (f64, f64) {
        let meter = self.location(0).map_or(4.0, |p| Self::meter_of(p.pattern));
        (self.bpm_at(0), meter)
    }

    /// The beats in a bar of `pattern`, counted in quarter notes.
    fn meter_of(pattern: &Pattern) -> f64 {
        pattern.timing.as_ref().map_or(4.0, |t| f64::from(t.beats_per_bar) * 4.0 / f64::from(t.beat_unit))
    }

    /// The next number from 0 up to 1 (splitmix64, seeded from the clock: nothing about the song fixes it).
    fn roll(&self) -> f64 {
        let state = self.random.get().wrapping_add(0x9E37_79B9_7F4A_7C15);
        self.random.set(state);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Whether the note in this row, track and column plays, given its chance. A note is rolled for
    /// once, whichever of its hold or its own row asks first.
    fn plays(&self, index: u64, track: usize, column: usize, chance: u32) -> bool {
        chance >= 100
            || *self.rolls.borrow_mut().entry((index, track, column)).or_insert_with(|| self.roll()) * 100.0 < f64::from(chance)
    }

    /// Apply edited sampler loop defaults to held and already queued notes.
    pub fn loop_edits(&self, instrument: &str, args: &Value, now: u64) -> Vec<Value> {
        self.sounding.iter().filter(|note| note.instrument == instrument && note.until > now)
            .map(|note| json!({"op":"set", "id":note.id, "at":now.max(note.begins + 1), "tag":"tracker-pattern", "args":args})).collect()
    }

    /// The tempo in beats a minute at row `index`.
    fn bpm_at(&self, index: u64) -> f64 {
        match self.location(index) {
            Some(p) => p.pattern.timing.as_ref().map_or(self.song.bpm, |timing| timing.bpm_at(p.row as f64)),
            None => self.song.bpm,
        }
    }

    /// The seconds from the start of the row at `index` to `fraction` of the way through it. This is the
    /// one place the tracker's time is made: swing moves time between the two rows of a pair, so notes,
    /// delays, hits, bends, lanes, the playhead and export all follow it. Pairs begin at each pattern's
    /// first row, and an odd last row has no partner and is left alone.
    fn row_seconds(&self, index: u64, fraction: f64) -> f64 {
        let Some(p) = self.location(index) else {
            return self.song.row_samples(1.0) * fraction;
        };
        let straight = |from: usize, rows: f64| match &p.pattern.timing {
            Some(timing) => timing.seconds(from as f64, from as f64 + rows),
            None => self.song.row_samples(1.0) * rows,
        };
        let swing = p.pattern.swing.unwrap_or(self.song.swing) / 100.0;
        let first = p.row & !1;
        if swing <= 0.0 || first + 1 >= p.pattern.rows {
            return straight(p.row, fraction);
        }
        let (a, b) = (straight(first, 1.0), straight(first + 1, 1.0));
        let moved = a.min(b) * swing;
        let length = if p.row == first { a + moved } else { b - moved };
        straight(p.row, fraction) * length / if p.row == first { a } else { b }
    }

    fn seconds_at(&self, row: f64) -> f64 {
        let index = row.floor() as usize;
        while self.seconds.borrow().len() <= index {
            let i = self.seconds.borrow().len() - 1;
            let next = self.seconds.borrow()[i] + self.row_seconds(i as u64, 1.0);
            self.seconds.borrow_mut().push(next);
        }
        self.seconds.borrow()[index] + self.row_seconds(index as u64, row.fract())
    }

    pub fn row_at(&self, index: u64, sample_rate: f64) -> u64 {
        self.origin + (self.seconds_at(index as f64) * sample_rate).round() as u64
    }

    fn row_index(&self, sample: u64, sample_rate: f64) -> u64 {
        let mut hi = self.next_row.max(1);
        while self.row_at(hi, sample_rate) <= sample { hi *= 2; }
        let mut lo = 0;
        while lo + 1 < hi {
            let mid = lo + (hi-lo)/2;
            if self.row_at(mid, sample_rate) <= sample { lo = mid; } else { hi = mid; }
        }
        lo
    }

    /// Keep timestamps already submitted to the engine; new timing starts at the next unscheduled row.
    pub fn update_song(&mut self, song: Song) {
        self.seconds_at(self.next_row as f64);
        self.seconds.borrow_mut().truncate(self.next_row as usize + 1);
        self.song = song;
    }

    pub fn location(&self, index: u64) -> Option<Position<'_>> {
        if self.mode == Mode::Pattern {
            let pattern = self
                .song
                .patterns
                .iter()
                .find(|p| p.id == self.pattern_id)?;
            return Some(Position {
                row: index as usize % pattern.rows,
                pattern,
                sequence_position: None,
            });
        }
        // With a loop, the first pass runs from the start through the loop's end, then the loop repeats.
        let looped = self.song.loop_range.filter(|l| l.end >= self.start_position);
        let mut row = index;
        let first_end = looped.map_or(self.song.sequence.len(), |l| l.end + 1);
        if let Some(found) = self.walk(self.start_position..first_end, &mut row) {
            return Some(found);
        }
        let l = looped?;
        let cycle = (l.start..=l.end)
            .map(|p| self.rows_at(p))
            .sum::<Option<u64>>()?;
        row %= cycle.max(1);
        self.walk(l.start..l.end + 1, &mut row)
    }

    fn rows_at(&self, sequence_position: usize) -> Option<u64> {
        let id = self.song.sequence.get(sequence_position)?;
        Some(self.song.patterns.iter().find(|p| p.id == *id)?.rows as u64)
    }

    /// The row `row` rows into the sequence positions `range`, or None after taking away the rows it passed.
    fn walk(&self, range: std::ops::Range<usize>, row: &mut u64) -> Option<Position<'_>> {
        for sequence_position in range {
            let id = self.song.sequence.get(sequence_position)?;
            let pattern = self.song.patterns.iter().find(|p| p.id == *id)?;
            if *row < pattern.rows as u64 {
                return Some(Position {
                    pattern,
                    row: *row as usize,
                    sequence_position: Some(sequence_position),
                });
            }
            *row -= pattern.rows as u64;
        }
        None
    }

    /// Changes the loop while playing without a jump: playback carries on from the start of the
    /// pattern now sounding, and loops from there.
    pub fn set_loop(&mut self, range: Option<LoopRange>, sample: u64, sample_rate: f64) {
        if self.mode == Mode::Song && sample >= self.origin {
            let index = self.row_index(sample, sample_rate);
            let now = self
                .location(index)
                .and_then(|p| Some((p.sequence_position?, p.row as u64)));
            if let Some((position, row)) = now {
                let start = index - row;
                self.origin = self.row_at(start, sample_rate);
                self.start_position = position;
                self.next_row = self.next_row.saturating_sub(start);
                let base = self.seconds.borrow()[start as usize];
                self.seconds.borrow_mut().drain(..start as usize);
                for value in self.seconds.borrow_mut().iter_mut() { *value -= base; }
            }
        }
        self.seconds.borrow_mut().truncate(self.next_row as usize + 1);
        self.song.loop_range = range;
    }

    pub fn position(&self, sample: u64, sample_rate: f64) -> Option<Position<'_>> {
        if sample < self.origin {
            return None;
        }
        let index = self.row_index(sample, sample_rate);
        self.location(index)
    }

    /// How far through the current row `sample` is, from 0 up to 1.
    pub fn phase(&self, sample: u64, sample_rate: f64) -> Option<f64> {
        if sample < self.origin {
            return None;
        }
        let index = self.row_index(sample, sample_rate);
        let start = self.row_at(index, sample_rate);
        let end = self.row_at(index + 1, sample_rate);
        Some((sample - start) as f64 / (end - start).max(1) as f64)
    }

    pub fn finished(&self, sample: u64, sample_rate: f64) -> bool {
        self.mode == Mode::Song
            && sample >= self.origin
            && self.position(sample, sample_rate).is_none()
    }

    /// The pattern `track` plays from at `position`: the step's own, or another one if the matrix says
    /// so. None when the track is muted there, or the other pattern has run out of rows.
    fn track_pattern<'a>(&'a self, position: &Position<'a>, track: usize) -> Option<&'a Pattern> {
        let source = position
            .sequence_position
            .and_then(|step| self.song.matrix.get(step))
            .and_then(|step| step.get(track))
            .and_then(Option::as_ref);
        match source {
            None => Some(position.pattern),
            Some(TrackSource::Mute) => None,
            Some(TrackSource::Pattern { id }) => self
                .song
                .patterns
                .iter()
                .find(|p| p.id == *id)
                .filter(|p| position.row < p.rows),
        }
    }

    /// How a note started on row `index` (itself `delay` rows late) is held: until the column next has
    /// a note or an OFF, or until the song or pattern runs out. `held` is the note's instrument and
    /// pitch when it can glide: a glide note after it then carries the sound on rather than ending
    /// it, and a slide command in a row below it bends the pitch.
    fn hold(&self, index: u64, track: usize, column: usize, delay: f64, held: Option<(&str, f64)>) -> Hold {
        const LOOKAHEAD: u64 = 16384;
        let mut hold = Hold { rows: 0.0, cut: false, bends: Vec::new(), changes: Vec::new(), absorbed: Vec::new() };
        let mut current = held.map(|(_, note)| note);
        let mut volume = 128.0;
        let ended = |mut hold: Hold, rows: f64, cut: bool| {
            hold.rows = (rows - delay).max(MIN_HOLD);
            hold.cut = cut;
            hold
        };
        for ahead in 0..=LOOKAHEAD {
            let Some(position) = self.location(index + ahead) else {
                // The song ends here.
                return ended(hold, ahead as f64, false);
            };
            let Some(pattern) = self.track_pattern(&position, track) else {
                // The track is muted from here, so the note ends where the mute begins.
                return ended(hold, ahead as f64, false);
            };
            let Some(next) = pattern.notes[track].get(column) else {
                continue;
            };
            let row = position.row;
            let command = next.pitch(row);
            if ahead == 0 {
                volume = next.volume(row).map_or(128.0, f64::from);
            }
            if ahead > 0
                && let Some(note) = next.note(row)
            {
                if midi(note).is_some() && !self.plays(index + ahead, track, column, next.chance(row)) {
                    // a note that does not play leaves the sound before it as it was, and so do its commands
                    continue;
                }
                let glide = match (command, held, midi(note)) {
                    (Some(Pitch::Glide(speed)), Some((instrument, _)), Some(to)) if next.instrument(row) == Some(instrument) => Some((Pitch::glide_rows(speed, to - current.unwrap_or(to)), to)),
                    _ => None,
                };
                let Some((rows, to)) = glide else {
                    return ended(hold, ahead as f64 + next.delayed(row), note == "CUT");
                };
                hold.bends.push(Bend { index: index + ahead, delay: next.delayed(row), rows, midi: to });
                hold.absorbed.push((index + ahead, track, column));
                current = Some(to);
                continue;
            }
            if let Some(change) = next.fade(row) {
                volume = (volume + change).clamp(0.0, 128.0);
                hold.changes.push(Change { index: index + ahead, delay: if ahead == 0 { delay } else { 0.0 }, volume });
            }
            if let (Some(Pitch::Slide(semitones)), Some(from)) = (command, current) {
                let to = from + semitones;
                let begins = if ahead == 0 { delay } else { 0.0 };
                hold.bends.push(Bend { index: index + ahead, delay: begins, rows: 1.0, midi: to });
                current = Some(to);
            }
        }
        ended(hold, LOOKAHEAD as f64, false)
    }

    /// Whether any pattern has a lane on `track` for a parameter of `instrument`, so its notes are named
    /// and can be changed while they sound.
    fn listens(&self, track: usize, instrument: &str) -> bool {
        self.instrument_parameters.get(instrument).is_some_and(|params| params.iter().any(|p| p.name == "loop_start")) || self.song.patterns.iter().any(|p| {
            p.fx.get(track).is_some_and(|lanes| {
                lanes.iter().any(|l| matches!(&l.target, Some(crate::commands::FxTarget::Instrument { instrument: i, .. }) if i == instrument))
            })
        })
    }

    /// Changes the notes of `instrument` that are sounding on `track` to what an instrument-parameter lane
    /// says at `row`: a value set on the row, or the steps and ramps of an envelope across it.
    fn change_sounding(&self, sounding: &[Sounding], track: usize, lane: &FxLane, parameter: &crate::commands::Parameter, when: (usize, u64, f64), events: &mut Vec<Value>) {
        let (row, at, sample_rate) = when;
        let instrument = match &lane.target {
            Some(crate::commands::FxTarget::Instrument { instrument, .. }) => instrument,
            _ => return,
        };
        let mut set = |id: u64, at: u64, value: f64, glide: Option<f64>| {
            let mut event = json!({"op":"set", "id":id, "at":at, "tag":"tracker-pattern", "args":{ &parameter.name: parameter.argument(value.clamp(parameter.min, parameter.max)) }});
            if let Some(seconds) = glide {
                event["glide"] = json!(format!("{seconds:.6}sec"));
            }
            events.push(event);
        };
        let listeners = || sounding.iter().filter(|n| n.track == track && n.instrument == *instrument && n.until > at);
        if let Some(envelope) = lane.envelope.as_ref().filter(|e| e.enabled) {
            let db = parameter.unit.as_deref() == Some("db");
            let (start, end) = (row as f64, row as f64 + 1.0);
            let mut breaks = vec![start];
            breaks.extend(envelope.points.iter().filter(|p| p.row > start && p.row < end).map(|p| p.row));
            breaks.push(end);
            for pair in breaks.windows(2) {
                let Some(value) = envelope.value(pair[0], db) else { continue };
                let from = self.origin + (self.seconds_at(self.next_row as f64 + pair[0] - start) * sample_rate).round() as u64;
                let to = self.origin + (self.seconds_at(self.next_row as f64 + pair[1] - start) * sample_rate).round() as u64;
                for note in listeners() {
                    set(note.id, from, value, None);
                    if envelope.mode == "linear" && to > from + 1 && let Some(next) = envelope.value(pair[1], db) {
                        set(note.id, from + 1, next, Some((to - from - 1) as f64 / sample_rate));
                    }
                }
            }
        }
    }

    /// The engine events for one automation lane at one row of a pattern: the steps of its envelope
    /// across the row, for the strip named `slot` (a track's number, `r<n>` or `m`).
    /// `when` is the pattern's length and current row.
    fn lane_events(
        &self,
        slot: &str,
        strip: &Track,
        lane: &FxLane,
        when: (usize, usize),
        sample_rate: f64,
        events: &mut Vec<Value>,
    ) -> Result<(), String> {
        let (rows, row) = when;
        if lane
            .target
            .as_ref()
            .is_some_and(|target| !target.available(strip, &self.parameters))
        {
            return Ok(());
        }
        if let (Some(envelope), Some(target)) = (&lane.envelope, &lane.target)
            && envelope.enabled
        {
            let start = row as f64;
            let end = start + 1.0;
            let mut breaks = vec![start];
            breaks.extend(
                envelope
                    .points
                    .iter()
                    .filter(|p| p.row > start && p.row < end)
                    .map(|p| p.row),
            );
            breaks.push(end);
            let db = match target {
                crate::commands::FxTarget::Volume => true,
                crate::commands::FxTarget::Effect { .. } => target
                    .parameter(strip, &self.parameters)
                    .is_some_and(|p| p.unit.as_deref() == Some("db")),
                crate::commands::FxTarget::Pan
                | crate::commands::FxTarget::Macro { .. }
                | crate::commands::FxTarget::Instrument { .. } => false,
            };
            for pair in breaks.windows(2) {
                let Some(value) = envelope.value(pair[0], db) else {
                    continue;
                };
                let point_at = self.origin + (self.seconds_at(self.next_row as f64 + pair[0] - start) * sample_rate).round() as u64;
                let end_at = self.origin + (self.seconds_at(self.next_row as f64 + pair[1] - start) * sample_rate).round() as u64;
                let mut point_edits: Vec<_> =
                    target.unmap(slot, strip).into_iter().collect();
                point_edits.extend(crate::commands::edit(
                    target,
                    &value.to_string(),
                    slot,
                    strip,
                    &self.parameters,
                )?);
                events.push(json!({"op":"routing", "at":point_at, "tag":"tracker-pattern", "edits":point_edits}));
                if envelope.mode == "linear" && end_at > point_at + 1 {
                    let value = envelope.value(pair[1], db).unwrap();
                    let mut ramp = crate::commands::edit(
                        target,
                        &value.to_string(),
                        slot,
                        strip,
                        &self.parameters,
                    )?;
                    for edit in &mut ramp { edit["glide"] = json!((end_at - point_at - 1) as f64 / sample_rate); }
                    events.push(json!({"op":"routing", "at":point_at+1, "tag":"tracker-pattern", "edits":ramp}));
                }
            }
            if envelope.mode == "hold"
                && row + 1 == rows
                && let Some(point) = envelope
                    .points
                    .iter()
                    .find(|p| p.row == rows as f64)
            {
                let edit = crate::commands::edit(
                    target,
                    &point.value.to_string(),
                    slot,
                    strip,
                    &self.parameters,
                )?;
                events.push(json!({"op":"routing", "at":self.row_at(self.next_row+1, sample_rate), "tag":"tracker-pattern", "edits":edit}));
            }
            return Ok(());
        }
        Ok(())
    }

    /// The edits the track commands in `columns` make at `row`.
    fn command_edits<'a>(&self, columns: impl IntoIterator<Item = &'a Vec<Option<String>>>, slot: &str, strip: &Track, row: usize, edits: &mut Vec<Value>) -> Result<(), String> {
        for cells in columns {
            let Some(command) = cells.get(row).and_then(|c| c.as_deref()).and_then(TrackCommand::parse) else {
                continue;
            };
            let Some((target, value)) = command.target(strip, &self.parameters) else {
                continue;
            };
            if !target.available(strip, &self.parameters) {
                continue;
            }
            edits.extend(target.unmap(slot, strip));
            edits.extend(crate::commands::edit(&target, &value.to_string(), slot, strip, &self.parameters)?);
        }
        Ok(())
    }

    // Derive timestamps from one origin, including across pattern boundaries.
    pub fn events_until(
        &mut self,
        sample: u64,
        horizon: u64,
        sample_rate: f64,
        targets: &BTreeMap<String, String>,
    ) -> Result<(Vec<Value>, u64), String> {
        let mut events = Vec::new();
        let mut skipped = 0;
        // Held apart from `self` so rows can be read while it is updated.
        let mut state = std::mem::take(&mut self.lane_state);
        let mut absorbed = std::mem::take(&mut self.absorbed);
        let mut sounding = std::mem::take(&mut self.sounding);
        state.resize_with(self.song.tracks.len(), BTreeMap::new);
        while self.row_at(self.next_row, sample_rate) < horizon {
            let at = self.row_at(self.next_row, sample_rate);
            let Some(position) = self.location(self.next_row) else {
                break;
            };
            if at < sample {
                skipped += 1;
                self.next_row += 1;
                continue;
            }
            sounding.retain(|note| note.until > at);
            // what an effect reads as `clock.bpm`, and what a length in beats lasts, follows the song's tempo as it
            // moves along a ramp or changes with the pattern
            let meter = Self::meter_of(position.pattern);
            let bpm = self.bpm_at(self.next_row);
            if self.told_tempo.get().is_none_or(|(told, bar)| (told - bpm).abs() > 0.01 || bar != meter) {
                events.push(json!({"op":"tempo", "at":at, "bpm":bpm, "meter":meter, "tag":"tracker-pattern"}));
                self.told_tempo.set(Some((bpm, meter)));
            }
            events.extend(crate::modulation::timing(&self.song, &self.parameters, self.row_seconds(self.next_row, 1.0), at));
            for (index, track) in self.song.tracks.iter().enumerate() {
                let Some(pattern) = self.track_pattern(&position, index) else {
                    continue;
                };
                let mut edits = Vec::new();
                for lane in &pattern.fx[index] {
                    if let Some(crate::commands::FxTarget::Instrument {
                        instrument,
                        parameter,
                    }) = &lane.target
                    {
                        // Remembered until changed, and given to each note of that instrument as it starts.
                        let info = self
                            .instrument_parameters
                            .get(instrument)
                            .and_then(|all| all.iter().find(|p| p.name == *parameter));
                        let value = match &lane.envelope {
                            Some(envelope) if envelope.enabled => envelope.value(
                                position.row as f64,
                                info.is_some_and(|p| p.unit.as_deref() == Some("db")),
                            ),
                            _ => None,
                        };
                        if let Some(value) = value {
                            state[index].insert((instrument.clone(), parameter.clone()), value);
                        }
                        if let Some(info) = info {
                            self.change_sounding(&sounding, index, lane, info, (position.row, at, sample_rate), &mut events);
                        }
                        continue;
                    }
                    self.lane_events(
                        &index.to_string(),
                        track,
                        lane,
                        (pattern.rows, position.row),
                        sample_rate,
                        &mut events,
                    )?;
                }
                self.command_edits(pattern.notes[index].iter().flat_map(|column| &column.fx), &index.to_string(), track, position.row, &mut edits)?;
                if !edits.is_empty() {
                    events.push(
                        json!({"op":"routing", "at":at, "tag":"tracker-pattern", "edits":edits}),
                    );
                }
                for (column_index, column) in pattern.notes[index].iter().enumerate() {
                    let Some(note) = column.note(position.row) else {
                        continue;
                    };
                    if note == "OFF" || note == "CUT" {
                        // It ends the note before it; that note was timed to stop here.
                        continue;
                    }
                    if absorbed.remove(&(self.next_row, index, column_index)) {
                        // A glide from the note before it carries the sound on.
                        continue;
                    }
                    if !self.plays(self.next_row, index, column_index, column.chance(position.row)) {
                        continue;
                    }
                    // A note with no instrument set at or above it is silent, not an error.
                    let Some(id) = column.instrument(position.row) else {
                        continue;
                    };
                    let target = targets
                        .get(id)
                        .ok_or_else(|| format!("Apply the instrument played on {} first", track.name))?;
                    let delay = column.delayed(position.row);
                    let slices = self.instrument_slices.get(id).copied();
                    let Some(mut args) = note_args(note, slices)? else {
                        continue;
                    };
                    let held = if slices.is_some() { None } else { midi(note).map(|m| (id, m)) };
                    let hold = self.hold(self.next_row, index, column_index, delay, held);
                    absorbed.extend(hold.absorbed.iter().copied());
                    if let Some(volume) = column.volume(position.row) {
                        args["gain"] = json!(format!("{:.3}db", volume_db(volume)));
                    }
                    if let Some(pan) = column.panning(position.row) {
                        args["pan"] = json!(format!("{:.3}deg", pan_degrees(pan)));
                    }
                    for parameter in self.instrument_parameters.get(id).into_iter().flatten() {
                        let value = state[index].get(&(id.to_string(), parameter.name.clone())).or_else(|| {
                            self.song
                                .instrument_values
                                .get(id)
                                .and_then(|values| values.get(&parameter.name))
                        });
                        if let Some(value) = value {
                            args[&parameter.name] =
                                parameter.argument(value.clamp(parameter.min, parameter.max));
                        }
                    }
                    fit_to_tempo(&mut args, self.instrument_parameters.get(id).map_or(&[], Vec::as_slice), self.bpm_at(self.next_row));
                    start_args(&mut args, column.start(position.row), self.instrument_parameters.get(id).map_or(&[], Vec::as_slice));
                    let retrigger = column.retrigger(position.row);
                    let hits = retrigger.map_or(1, |r| r.hits);
                    for hit in 0..hits {
                        let hit_args = retrigger.map_or_else(|| args.clone(), |r| hit_args(&args, r.gain(hit), self.instrument_parameters.get(id).map_or(&[], Vec::as_slice)));
                        let last = hit + 1 == hits;
                        let offset = f64::from(hit) / f64::from(hits);
                        let rows = if last { hold.rows - offset } else { 1.0 / f64::from(hits) };
                        let rows = column.length(position.row).map_or(rows, |len| rows.min(len / f64::from(hits))).max(MIN_HOLD);
                        let begins = self.origin + (self.seconds_at(self.next_row as f64 + delay + offset) * sample_rate).round() as u64;
                        let mut note_event = json!({"op":"note", "at":begins, "target":target, "to":format!("in_{index}"), "tag":"tracker-pattern",
                            "dur":format!("{:.6}sec", self.seconds_at(self.next_row as f64 + delay + offset + rows) - self.seconds_at(self.next_row as f64 + delay + offset)), "args":hit_args});
                        // A cut ends the sound where the note ends, with no release, and a bend changes its
                        // pitch while it sounds: either way the note is given an id to find it by.
                        let changes = last && self.listens(index, id);
                        let named = last && (hold.cut || !hold.bends.is_empty() || !hold.changes.is_empty()) || changes;
                        let note_id = named.then(|| NOTE_IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
                        if let Some(id) = note_id {
                            note_event["id"] = json!(id);
                        }
                        events.push(note_event);
                        let Some(note_id) = note_id else { continue };
                        let end = self.origin + (self.seconds_at(self.next_row as f64 + delay + offset + rows) * sample_rate).round() as u64;
                        if changes {
                            sounding.push(Sounding { id: note_id, begins, track: index, instrument: id.to_string(), until: end });
                        }
                        let id = note_id;
                        for bend in &hold.bends {
                            let from = self.origin + (self.seconds_at(bend.index as f64 + bend.delay) * sample_rate).round() as u64;
                            events.push(json!({"op":"set", "id":id, "at":from.max(begins + 1), "tag":"tracker-pattern",
                                "args":{"freq":format!("{:.3}hz", hertz(bend.midi))}, "glide":format!("{:.6}sec", (self.seconds_at(bend.index as f64 + bend.delay + bend.rows) - self.seconds_at(bend.index as f64 + bend.delay)).max(0.001))}));
                        }
                        for change in &hold.changes {
                            let from = self.origin + (self.seconds_at(change.index as f64 + change.delay) * sample_rate).round() as u64;
                            let args = json!({"gain": format!("{:.3}db", volume_db(change.volume.round() as u32))});
                            events.push(json!({"op":"set", "id":id, "at":from.max(begins + 1), "tag":"tracker-pattern", "args":args,
                                "glide":format!("{:.6}sec", (self.seconds_at(change.index as f64 + change.delay + 1.0) - self.seconds_at(change.index as f64 + change.delay)).max(0.001))}));
                        }
                        if hold.cut {
                            // the fall takes 2 ms and ends where the note does, since a note that has been released ignores it
                            let fall = (sample_rate * 0.002) as u64;
                            events.push(json!({"op":"set", "id":id, "at":end.saturating_sub(fall).max(begins), "tag":"tracker-pattern",
                                "args":{"gain":"-120db"}, "glide":"2ms"}));
                        }
                    }
                }
            }
            // returns and the master follow the step's own pattern, whatever the matrix says of the tracks
            let pattern = position.pattern;
            let strips = self
                .song
                .returns
                .iter()
                .zip(&pattern.return_fx)
                .enumerate()
                .map(|(r, (strip, lanes))| (format!("r{r}"), strip, lanes))
                .chain([(crate::mixer::MASTER.to_string(), &self.song.master, &pattern.master_fx)]);
            let mut edits = Vec::new();
            for (at_strip, (slot, strip, lanes)) in strips.enumerate() {
                for lane in lanes {
                    self.lane_events(&slot, strip, lane, (pattern.rows, position.row), sample_rate, &mut events)?;
                }
                self.command_edits(pattern.commands.get(at_strip).into_iter().flatten(), &slot, strip, position.row, &mut edits)?;
            }
            if !edits.is_empty() {
                events.push(json!({"op":"routing", "at":at, "tag":"tracker-pattern", "edits":edits}));
            }
            self.next_row += 1;
        }
        self.lane_state = state;
        absorbed.retain(|(row, ..)| *row >= self.next_row);
        self.rolls.borrow_mut().retain(|(row, ..), _| *row >= self.next_row);
        self.absorbed = absorbed;
        self.sounding = sounding;
        Ok((events, skipped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn song() -> Song {
        Song {
            bpm: 137.0,
            rows_per_beat: 4,
            swing: 0.0,
            tracks: (0..4)
                .map(|i| Track {
                    name: format!("Track {i}"),
                    ..Track::default()
                })
                .collect(),
            patterns: [("a", 2, "C-4"), ("b", 3, "D#4")]
                .into_iter()
                .map(|(id, rows, note)| Pattern {
                    timing: None,
                    swing: None,
                    id: id.into(),
                    name: id.into(),
                    rows,
                    notes: (0..4)
                        .map(|track| {
                            vec![NoteColumn {
                                notes: vec![Some(note.into()); rows],
                                // only the first track has an instrument, so only it sounds
                                inst: if track == 0 { vec![Some("tone".into()); rows] } else { vec![] },
                                ..NoteColumn::default()
                            }]
                        })
                        .collect(),
                    fx: (0..4).map(|_| vec![FxLane::empty()]).collect(),
                    return_fx: vec![],
                    master_fx: vec![FxLane::empty()],
                    commands: vec![],
                })
                .collect(),
            sequence: vec!["a".into(), "b".into(), "a".into()],
            loop_range: None,
            matrix: vec![],
            master: Track::default(),
            returns: vec![],
            instrument_values: Default::default(),
        }
    }
    /// The notes track 0 plays over the whole of `song`, as (frequency, duration in seconds).
    fn track_zero(song: &Song) -> Vec<(String, f64)> {
        let mut play = Playback::new(0, song.clone(), Mode::Song, "a".into(), 0).unwrap();
        let targets = BTreeMap::from([("tone".into(), "voice".into())]);
        let horizon = play.row_at(100, 48000.0);
        let (events, _) = play.events_until(0, horizon, 48000.0, &targets).unwrap();
        events
            .iter()
            .filter(|e| e["op"] == "note" && e["to"] == "in_0")
            .map(|e| (e["args"]["freq"].as_str().unwrap().to_string(), e["dur"].as_str().unwrap().trim_end_matches("sec").parse().unwrap()))
            .collect()
    }
    fn freqs(notes: &[(String, f64)]) -> Vec<&str> {
        notes.iter().map(|(f, _)| f.as_str()).collect()
    }

    fn timing(bpm: f64) -> Timing {
        Timing { bpm, rows_per_beat: 4, beats_per_bar: 7, beat_unit: 8, tempo: vec![] }
    }

    #[test]
    fn tempo_ramps_integrate_and_pattern_changes_time_notes_across_boundaries() {
        let mut song = song();
        song.patterns[0].timing = Some(timing(120.0));
        song.patterns[1].timing = Some(timing(60.0));
        song.patterns[0].notes[0][0].notes = vec![Some("C-4".into()), None];
        song.patterns[1].notes[0][0].notes = vec![None, Some("OFF".into()), None];
        let mut play = Playback::new(0, song, Mode::Song, "a".into(), 0).unwrap();
        assert_eq!(play.row_at(2, 48000.0), 12000);
        assert_eq!(play.row_at(3, 48000.0), 24000);
        assert_eq!(play.position(18000, 48000.0).unwrap().row, 0);
        assert!((play.phase(18000, 48000.0).unwrap() - 0.5).abs() < 1e-9);
        let targets = BTreeMap::from([("tone".into(), "voice".into())]);
        let (events, _) = play.events_until(0, 1, 48000.0, &targets).unwrap();
        assert_eq!(events.iter().find(|e| e["op"] == "note").unwrap()["dur"], "0.500000sec");
        let mut t = timing(120.0);
        t.tempo = vec![EnvelopePoint { row: 0.0, value: 120.0 }, EnvelopePoint { row: 4.0, value: 60.0 }];
        assert!((t.seconds(0.0, 4.0) - 2.0_f64.ln()).abs() < 1e-9);
        assert!((t.seconds(0.0, 2.5) + t.seconds(2.5, 4.0) - t.seconds(0.0, 4.0)).abs() < 1e-9);
    }

    #[test]
    fn the_engine_is_told_of_a_tempo_ramp_and_of_a_pattern_with_its_own_tempo() {
        let mut song = song();
        let mut ramp = timing(120.0);
        ramp.beats_per_bar = 4;
        ramp.beat_unit = 4;
        ramp.tempo = vec![EnvelopePoint { row: 0.0, value: 120.0 }, EnvelopePoint { row: 2.0, value: 60.0 }];
        song.patterns[0].timing = Some(ramp);
        song.patterns[1].timing = Some(Timing { bpm: 90.0, rows_per_beat: 4, beats_per_bar: 3, beat_unit: 4, tempo: vec![] });
        let mut play = Playback::new(0, song, Mode::Song, "a".into(), 0).unwrap();
        let targets = BTreeMap::from([("tone".into(), "voice".into())]);
        let (events, _) = play.events_until(0, 200_000, 48000.0, &targets).unwrap();
        let tempos: Vec<(f64, f64)> = events.iter().filter(|e| e["op"] == "tempo").map(|e| (e["bpm"].as_f64().unwrap(), e["meter"].as_f64().unwrap())).collect();
        // the first row's 120 is where it starts. Then: the second row of the ramp is halfway down it, the second
        // pattern has its own 90 in 3/4, and the first comes round again from 120 and halfway down once more
        assert_eq!(play.start_tempo(), (120.0, 4.0));
        assert_eq!(tempos, [(90.0, 4.0), (90.0, 3.0), (120.0, 4.0), (90.0, 4.0)]);
    }

    #[test]
    fn live_tempo_edits_keep_queued_rows_and_retime_future_rows() {
        let mut song = song(); song.bpm = 120.0;
        let mut play = Playback::new(0, song.clone(), Mode::Pattern, "a".into(), 0).unwrap();
        play.next_row = 2;
        play.row_at(100, 48000.0); // note lookahead can have cached the future
        song.bpm = 60.0;
        assert!(play.song.same_timeline(&song));
        play.update_song(song);
        assert_eq!(play.row_at(2, 48000.0), 12000);
        assert_eq!(play.row_at(3, 48000.0), 24000);
        assert_eq!(play.position(18000, 48000.0).unwrap().row, 0);
        play.set_loop(None, 18000, 48000.0);
        assert_eq!(play.row_at(3, 48000.0), 24000);
    }

    #[test]
    fn invalid_tempo_points_and_meters_are_rejected() {
        let mut song = song();
        let mut t = timing(120.0); t.beat_unit = 3;
        song.patterns[0].timing = Some(t);
        assert!(song.validate().is_err());
        let mut t = timing(120.0);
        t.tempo = vec![EnvelopePoint { row: 3.0, value: 90.0 }];
        song.patterns[0].timing = Some(t);
        assert!(song.validate().is_err());
    }

    #[test]
    fn the_matrix_mutes_or_redirects_one_track_in_one_step() {
        // a (2 rows) and b (3 rows) have notes on every row; sequence a, b, a.
        let mut s = song();
        assert_eq!(freqs(&track_zero(&s)), ["c4", "c4", "d#4", "d#4", "d#4", "c4", "c4"]);
        // Muted in step 1: only a's rows sound. Other tracks and other steps are untouched.
        s.matrix = vec![vec![], vec![Some(TrackSource::Mute)]];
        assert_eq!(freqs(&track_zero(&s)), ["c4", "c4", "c4", "c4"]);
        s.matrix = vec![vec![None, Some(TrackSource::Mute)]];
        assert_eq!(track_zero(&s).len(), 7, "track 1 is muted, not track 0");
        // Redirected: step 0 plays b's notes, and step 1 plays a's, which has run out after two rows.
        s.matrix = vec![
            vec![Some(TrackSource::Pattern { id: "b".into() })],
            vec![Some(TrackSource::Pattern { id: "a".into() })],
        ];
        assert_eq!(freqs(&track_zero(&s)), ["d#4", "d#4", "c4", "c4", "c4", "c4"]);
        // Pattern mode ignores the matrix.
        s.matrix = vec![vec![Some(TrackSource::Mute)]];
        let play = Playback::new(0, s.clone(), Mode::Pattern, "a".into(), 0).unwrap();
        assert_eq!(play.track_pattern(&play.location(0).unwrap(), 0).unwrap().id, "a");
        // The matrix is validated.
        assert!(s.validate().is_ok());
        s.matrix = vec![vec![Some(TrackSource::Pattern { id: "missing".into() })]];
        assert!(s.validate().is_err());
        s.matrix = vec![vec![]; 4];
        assert!(s.validate().is_err(), "more steps than the sequence");
        s.matrix = vec![vec![None; 5]];
        assert!(s.validate().is_err(), "more tracks than the song");
        // It can change while playing; the timeline does not.
        assert!(song().same_timeline(&s));
    }

    #[test]
    fn a_note_ends_where_a_muted_step_begins() {
        // a: two rows with a note on the first; b: three silent rows.
        let mut s = song();
        s.bpm = 120.0;
        s.sequence = vec!["a".into(), "b".into()];
        s.patterns[0].notes[0][0].notes = vec![Some("C-4".into()), None];
        s.patterns[1].notes[0][0].notes = vec![None; 3];
        let held = track_zero(&s);
        assert_eq!(held.len(), 1);
        assert!((held[0].1 - 5.0 * 0.125).abs() < 1e-9, "rings to the end of the song: {held:?}");
        s.matrix = vec![vec![], vec![Some(TrackSource::Mute)]];
        let cut = track_zero(&s);
        assert!((cut[0].1 - 2.0 * 0.125).abs() < 1e-9, "stops at the mute: {cut:?}");
        // A track that falls silent because its pattern is shorter stops there too.
        s.matrix = vec![vec![], vec![Some(TrackSource::Pattern { id: "a".into() })]];
        let alias = track_zero(&s);
        assert_eq!(alias.len(), 2, "the alias plays a's note on step 1's first row");
        assert!((alias[1].1 - 2.0 * 0.125).abs() < 1e-9, "and a has only two rows, so it stops after them: {alias:?}");
    }

    #[test]
    fn a_loop_repeats_its_positions_and_moving_it_carries_on_from_the_pattern_now_playing() {
        let seq = |play: &Playback, row: u64| play.location(row).unwrap().sequence_position;
        // a (2 rows), b (3 rows), a (2 rows): rows 0-1, 2-4, 5-6.
        let mut looped = song();
        looped.loop_range = Some(LoopRange { start: 1, end: 1 });
        let play = Playback::new(0, looped.clone(), Mode::Song, "a".into(), 0).unwrap();
        assert_eq!((0..9).map(|r| seq(&play, r)).collect::<Vec<_>>(), [0, 0, 1, 1, 1, 1, 1, 1, 1].map(Some), "lead in, then b forever");
        assert_eq!(play.location(100).unwrap().pattern.id, "b");
        assert!(!play.finished(100, 48000.0));
        // Starting inside the loop plays the rest of it, then the whole loop again.
        looped.loop_range = Some(LoopRange { start: 0, end: 1 });
        let play = Playback::new(0, looped.clone(), Mode::Song, "b".into(), 1).unwrap();
        assert_eq!((0..10).map(|r| seq(&play, r)).collect::<Vec<_>>(), [1, 1, 1, 0, 0, 1, 1, 1, 0, 0].map(Some));
        // Starting after the loop ignores it.
        looped.loop_range = Some(LoopRange { start: 0, end: 0 });
        let play = Playback::new(0, looped.clone(), Mode::Song, "b".into(), 1).unwrap();
        assert_eq!(seq(&play, 3), Some(2));
        assert!(play.location(5).is_none());
        // The loop can change while playing; the sequence cannot.
        assert!(song().same_timeline(&looped));
        // Loop bounds are validated.
        looped.loop_range = Some(LoopRange { start: 2, end: 3 });
        assert!(looped.validate().is_err());
        looped.loop_range = Some(LoopRange { start: 2, end: 1 });
        assert!(looped.validate().is_err());

        // Turning a loop on while playing carries on from the start of the pattern now sounding.
        let mut play = Playback::new(1000, song(), Mode::Song, "a".into(), 0).unwrap();
        play.next_row = 4;
        let sample = play.row_at(3, 48000.0) + 10; // the second row of b
        play.set_loop(Some(LoopRange { start: 1, end: 1 }), sample, 48000.0);
        assert_eq!(play.start_position, 1);
        assert_eq!(play.next_row, 2, "the cursor keeps its place, relative to the pattern's first row");
        assert_eq!(play.origin, 1000 + (2.0 * play.song.row_samples(48000.0)).round() as u64);
        assert_eq!((0..7).map(|r| seq(&play, r)).collect::<Vec<_>>(), [1; 7].map(Some));
        // Turning it off lets the song run on from there.
        play.set_loop(None, play.row_at(4, 48000.0), 48000.0);
        assert_eq!(play.start_position, 1);
        assert_eq!((0..5).map(|r| seq(&play, r)).collect::<Vec<_>>(), [1, 1, 1, 2, 2].map(Some));
        assert!(play.location(5).is_none());
    }
    #[test]
    fn sequence_reuses_patterns_and_crosses_variable_length_boundaries_without_drift() {
        let mut play = Playback::new(4800, song(), Mode::Song, "a".into(), 0).unwrap();
        let targets = BTreeMap::from([("tone".into(), "voice".into())]);
        let horizon = play.row_at(8, 48000.0);
        let (events, skipped) = play.events_until(4800, horizon, 48000.0, &targets).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(events.len(), 7);
        assert_eq!(
            events
                .iter()
                .map(|e| e["args"]["freq"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["c4", "c4", "d#4", "d#4", "d#4", "c4", "c4"]
        );
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event["at"], play.row_at(i as u64, 48000.0));
        }
        assert_eq!(
            play.position(play.row_at(2, 48000.0) + 1, 48000.0)
                .unwrap()
                .sequence_position,
            Some(1)
        );
        assert!(play.finished(play.row_at(7, 48000.0) + 1, 48000.0));
    }
    /// A one-track pattern of `rows` rows with the given cells, played from row 0 at 120 bpm, 4 rows a beat.
    fn played(rows: usize, columns: Vec<NoteColumn>, mode: Mode) -> Vec<Value> {
        played_with(rows, columns, mode, |_| {})
    }
    fn played_with(rows: usize, columns: Vec<NoteColumn>, mode: Mode, setup: impl FnOnce(&mut Playback)) -> Vec<Value> {
        let mut song = song();
        song.bpm = 120.0;
        song.tracks.truncate(1);
        song.patterns = vec![Pattern {
            timing: None,
            swing: None,
            id: "a".into(),
            name: "a".into(),
            rows,
            notes: vec![columns],
            fx: vec![vec![FxLane::empty()]],
            return_fx: vec![],
            master_fx: vec![FxLane::empty()],
            commands: vec![],
        }];
        song.sequence = vec!["a".into()];
        let mut play = Playback::new(0, song, mode, "a".into(), 0).unwrap();
        setup(&mut play);
        let targets = BTreeMap::from([
            ("tone".into(), "voice".into()),
            ("alt".into(), "other".into()),
        ]);
        let horizon = play.row_at(rows as u64, 48000.0);
        // the notes and lanes: what is said of the tempo has a test of its own
        let events = play.events_until(0, horizon, 48000.0, &targets).unwrap().0;
        events.into_iter().filter(|e| e["op"] != "tempo").collect()
    }
    fn column(rows: usize, cells: &[(usize, &str)]) -> NoteColumn {
        let mut column = NoteColumn::blank(rows);
        column.inst = vec![Some("tone".into()); rows];
        for (row, note) in cells {
            column.notes[*row] = Some((*note).into());
        }
        column
    }
    fn duration(event: &Value) -> f64 {
        event["dur"]
            .as_str()
            .unwrap()
            .trim_end_matches("sec")
            .parse()
            .unwrap()
    }

    #[test]
    fn notes_are_held_until_the_next_note_or_off() {
        // 120 bpm at 4 rows a beat is an eighth of a second a row.
        let events = played(
            8,
            vec![column(8, &[(0, "C-4"), (3, "D-4"), (4, "OFF"), (6, "E-4")])],
            Mode::Song,
        );
        let held: Vec<_> = events.iter().map(duration).collect();
        assert_eq!(events.len(), 3, "OFF starts nothing");
        assert!((held[0] - 3.0 * 0.125).abs() < 1e-5, "{held:?}");
        assert!((held[1] - 0.125).abs() < 1e-5, "OFF ends D-4: {held:?}");
        assert!(
            (held[2] - 2.0 * 0.125).abs() < 1e-5,
            "until the song ends: {held:?}"
        );
        // Looping a pattern holds a lone note for the whole pattern.
        let looped = played(8, vec![column(8, &[(2, "C-4")])], Mode::Pattern);
        assert!((duration(&looped[0]) - 8.0 * 0.125).abs() < 1e-5);
    }

    #[test]
    fn delay_volume_pan_and_instrument_shape_each_note() {
        let mut first = column(4, &[(0, "C-4"), (2, "G-4")]);
        first.delay = vec![Some("128".into()), None, None, None];
        first.vol = vec![Some("64".into()), None, Some("0".into()), None];
        first.pan = vec![Some("0".into()), None, Some("128".into()), None];
        first.inst = vec![Some("tone".into()), None, Some("alt".into()), None];
        let events = played(4, vec![first], Mode::Song);
        let row = 6000;
        assert_eq!(events[0]["at"], row / 2, "half a row late");
        assert_eq!(events[0]["target"], "voice");
        assert_eq!(events[1]["target"], "other");
        assert_eq!(events[0]["args"]["gain"], "-6.021db");
        assert_eq!(events[1]["args"]["gain"], "-120.000db");
        assert_eq!(events[0]["args"]["pan"], "-30.000deg");
        assert_eq!(events[1]["args"]["pan"], "30.000deg");
        // The first note ends when the second begins, which is on time.
        assert!((duration(&events[0]) - (2.0 - 0.5) * 0.125).abs() < 1e-5);
        assert!(events[1]["args"].get("freq").is_some());
    }

    #[test]
    fn columns_play_together_and_hold_independently() {
        let events = played(
            4,
            vec![
                column(4, &[(0, "C-4"), (2, "OFF")]),
                column(4, &[(0, "E-4"), (1, "G-4")]),
            ],
            Mode::Song,
        );
        assert_eq!(events.len(), 3);
        let at_zero: Vec<_> = events.iter().filter(|e| e["at"] == 0).collect();
        assert_eq!(at_zero.len(), 2, "a chord starts together");
        let long = at_zero.iter().find(|e| e["args"]["freq"] == "c4").unwrap();
        let short = at_zero.iter().find(|e| e["args"]["freq"] == "e4").unwrap();
        assert!((duration(long) - 2.0 * 0.125).abs() < 1e-5);
        assert!((duration(short) - 0.125).abs() < 1e-5);
    }

    #[test]
    fn a_cut_ends_a_note_like_an_off_and_stops_its_sound_at_once() {
        let events = played(8, vec![column(8, &[(0, "C-4"), (3, "CUT"), (4, "D-4"), (6, "OFF")])], Mode::Song);
        let notes: Vec<_> = events.iter().filter(|e| e["op"] == "note").collect();
        assert_eq!(notes.len(), 2, "a CUT, like an OFF, starts nothing");
        assert!((duration(notes[0]) - 3.0 * 0.125).abs() < 1e-5, "held to the CUT");
        assert!((duration(notes[1]) - 2.0 * 0.125).abs() < 1e-5, "held to the OFF");
        let id = notes[0]["id"].as_u64().expect("a note that is cut has an id to find it by");
        assert!(notes[1].get("id").is_none(), "a note that is only released needs none");
        let set = events.iter().find(|e| e["op"] == "set").expect("the cut");
        assert_eq!(set["id"], id); assert_eq!(set["at"], notes[0]["at"].as_u64().unwrap() + 3 * 6000 - 96); // 2 ms before the note ends
        assert_eq!(set["args"]["gain"], "-120db"); assert_eq!(events.iter().filter(|e| e["op"] == "set").count(), 1);
    }

    fn parameter(name: &str) -> crate::commands::Parameter {
        crate::commands::Parameter { name: name.into(), min: 0.0, max: 1.0, default: 0.0, unit: None, ..Default::default() }
    }
    fn with_commands(rows: usize, cells: &[(usize, &str)], commands: &[(usize, &str)]) -> NoteColumn {
        let mut column = column(rows, cells);
        // the commands are written as S80, B00, R03, G10, U10 or D10, one to a row
        column.fx = vec![vec![None; rows]];
        for (row, text) in commands {
            column.fx[0][*row] = Some((*text).into());
        }
        column
    }

    #[test]
    fn a_repitch_fit_changes_the_speed_of_the_sampler_and_leaves_its_stretch_alone() {
        let fitted = |mode: f64, rate: Option<f64>| {
            let events = played_with(2, vec![column(2, &[(0, "C-4")])], Mode::Song, |play| {
                let ranged = |name: &str, min, max, default| crate::commands::Parameter { name: name.into(), min, max, default, unit: None, ..Default::default() };
                play.instrument_parameters.insert("tone".into(), vec![ranged("stretch", 0.25, 4.0, 1.0), ranged("rate", -4.0, 4.0, 1.0), ranged("from_bpm", 0.0, 400.0, 0.0), ranged("fit_mode", 0.0, 1.0, 0.0)]);
                let values = play.song.instrument_values.entry("tone".into()).or_default();
                values.insert("from_bpm".into(), 100.0); values.insert("fit_mode".into(), mode);
                if let Some(rate) = rate { values.insert("rate".into(), rate); }
            });
            (events[0]["args"]["rate"].as_f64(), events[0]["args"]["stretch"].as_f64())
        };
        // a 100 bpm sample in a 120 bpm song plays 1.2 times as fast, and backwards stays backwards
        let (rate, stretch) = fitted(1.0, None);
        assert!((rate.unwrap() - 1.2).abs() < 1e-9); assert_eq!(stretch, None);
        assert!((fitted(1.0, Some(-1.0)).0.unwrap() + 1.2).abs() < 1e-9);
        // stretch mode keeps the rate as it was, and stretches instead
        let (rate, stretch) = fitted(0.0, None);
        assert_eq!(rate, None); assert!((stretch.unwrap() - 100.0 / 120.0).abs() < 1e-9);
    }

    #[test]
    fn a_sampler_with_a_source_tempo_is_stretched_to_the_tempo_of_the_song() {
        let fitted = |from: f64, stretch: Option<f64>, timing: Option<Timing>| {
            let events = played_with(2, vec![column(2, &[(0, "C-4")])], Mode::Song, |play| {
                let ranged = |name: &str, min, max, default| crate::commands::Parameter { name: name.into(), min, max, default, unit: None, ..Default::default() };
                play.instrument_parameters.insert("tone".into(), vec![ranged("stretch", 0.25, 4.0, 1.0), ranged("from_bpm", 0.0, 400.0, 0.0)]);
                let values = play.song.instrument_values.entry("tone".into()).or_default();
                values.insert("from_bpm".into(), from);
                if let Some(stretch) = stretch { values.insert("stretch".into(), stretch); }
                play.song.patterns[0].timing = timing;
            });
            events[0]["args"]["stretch"].as_f64()
        };
        // a 140 bpm break in a 120 bpm song lasts 140/120 as long, and the stretch of its own multiplies that
        assert!((fitted(140.0, None, None).unwrap() - 140.0 / 120.0).abs() < 1e-9);
        assert!((fitted(140.0, Some(1.5), None).unwrap() - 1.75).abs() < 1e-9);
        // a pattern with a tempo of its own sets it
        assert!((fitted(120.0, None, Some(timing(60.0))).unwrap() - 2.0).abs() < 1e-9);
        // and the engine's range is kept
        assert_eq!(fitted(400.0, None, Some(timing(60.0))), Some(4.0)); assert_eq!(fitted(20.0, Some(0.5), Some(timing(300.0))), Some(0.25));
        // 0 leaves the sample alone, and an instrument without the parameters is not touched
        assert_eq!(fitted(0.0, Some(2.0), None), Some(2.0));
        let plain = played_with(2, vec![column(2, &[(0, "C-4")])], Mode::Song, |play| { play.instrument_parameters.insert("tone".into(), vec![parameter("from_bpm")]); });
        assert!(plain[0]["args"].get("stretch").is_none());
    }

    #[test]
    fn commands_are_a_letter_and_two_hex_digits() {
        assert_eq!(Command::parse("S80"), Some(Command { letter: 'S', value: 0x80 }));
        assert_eq!(Command::parse("RF4"), Some(Command { letter: 'R', value: 0xF4 }));
        for bad in ["", "S", "S8", "S800", "s80", "SG0", "80S", "S-1"] {
            assert_eq!(Command::parse(bad), None, "{bad}");
        }
        let column = |text: &str| with_commands(2, &[(0, "C-4")], &[(0, text)]);
        assert_eq!(column("S80").start(0), Start { offset: Some(0.5), reverse: false });
        assert_eq!(column("B00").start(0), Start { offset: None, reverse: true });
        assert_eq!(column("B01").start(0), Start::default());
        assert_eq!(column("G10").pitch(0), Some(Pitch::Glide(16.0)));
        assert_eq!(column("U10").pitch(0), Some(Pitch::Slide(12.0)));
        assert_eq!(column("D04").pitch(0), Some(Pitch::Slide(-3.0)));
        assert_eq!(column("R03").retrigger(0), Some(Retrigger { hits: 4, change: 0.0 }));
        assert_eq!(column("R76").retrigger(0), Some(Retrigger { hits: 2, change: -0.5 }));
        assert_eq!(column("RF4").retrigger(0), Some(Retrigger { hits: 3, change: 1.0 }));
        assert_eq!(column("C06").length(0), Some(0.5));
        assert_eq!(column("YFF").chance(0), 100);
        assert_eq!(column("Y80").chance(0), 50);
        assert_eq!(column("O40").fade(0), Some(-64.0));
        assert_eq!(column("I20").fade(0), Some(32.0));
        // 00 repeats the last value of its command above it in the column, and does nothing with none
        let mut repeating = with_commands(8, &[(0, "A-4")], &[(1, "U10"), (2, "D04"), (3, "U00"), (4, "D00"), (5, "I20"), (6, "I00")]);
        assert_eq!((repeating.pitch(3), repeating.pitch(4)), (Some(Pitch::Slide(12.0)), Some(Pitch::Slide(-3.0))));
        assert_eq!(repeating.fade(6), Some(32.0));
        repeating.fx[0][0] = Some("G00".into());
        assert_eq!(repeating.pitch(0), None);
        // a glide speed covers the distance at that many sixteenths of a semitone a tick; FF is at once
        assert_eq!(Pitch::glide_rows(16.0, 12.0), 1.0);
        assert_eq!(Pitch::glide_rows(8.0, -12.0), 2.0);
        assert_eq!(Pitch::glide_rows(255.0, 12.0), 0.0);
        for bad in ["Q10", "B02", "Y00", "O81", "R00", "R07", "C10", "C00", "C0C"] {
            assert!(Command::parse(bad).is_some_and(|c| c.problem().is_some()), "{bad}");
        }
        let mut song = song();
        song.patterns[0].notes[0][0].fx = vec![vec![Some("Q10".into()), None]];
        assert!(song.validate().is_err());
        song.patterns[0].notes[0][0].fx = vec![vec![Some("G10".into()), None]];
        song.validate().unwrap();
        song.patterns[0].notes[0][0].fx = vec![vec![Some("G10".into()), None], vec![Some("R03".into()), None]];
        assert!(song.validate().unwrap_err().contains("glide"), "a glide and a retrigger compete for the row");
        song.patterns[0].notes[0][0].fx = vec![vec![Some("U10".into()), None], vec![Some("R03".into()), None]];
        song.validate().unwrap();
        song.patterns[0].notes[0][0].fx = vec![vec![None, None]; MAX_FX_COLUMNS + 1];
        assert!(song.validate().is_err());
    }

    #[test]
    fn a_retrigger_plays_the_note_again_within_its_row() {
        // an eighth of a second a row: four hits are 31.25 ms apart, 1500 samples
        let events = played(4, vec![with_commands(4, &[(1, "C-4")], &[(1, "R03")])], Mode::Song);
        let at: Vec<_> = events.iter().filter(|e| e["op"] == "note").map(|e| e["at"].as_u64().unwrap()).collect();
        assert_eq!(at, [6000, 7500, 9000, 10500]);
        let held: Vec<_> = events.iter().map(duration).collect();
        assert!((held[0] - 0.03125).abs() < 1e-5, "{held:?}");
        assert!((held[3] - (3.0 * 0.125 - 0.09375)).abs() < 1e-5, "the last hit is held as the note would be: {held:?}");
    }

    #[test]
    fn each_hit_of_a_retrigger_is_a_set_share_quieter_or_louder_than_the_one_before() {
        let db = |e: &Value| e["args"].get("gain").and_then(|g| g.as_str()).map_or(0.0, |g| g.trim_end_matches("db").parse::<f64>().unwrap());
        let gains = |text: &str| -> Vec<f64> {
            played(2, vec![with_commands(2, &[(0, "C-4")], &[(0, text)])], Mode::Song).iter().map(db).collect()
        };
        let halving = 20.0 * 0.5_f64.log10();
        let fading = gains("R73");
        assert_eq!(fading.len(), 4);
        for (hit, gain) in fading.iter().enumerate() {
            assert!((gain - hit as f64 * halving).abs() < 0.01, "{fading:?}");
        }
        let swelling = gains("RF4");
        assert!((swelling[0] + 2.0 * -halving).abs() < 0.01 && (swelling[1] + -halving).abs() < 0.01 && swelling[2] == 0.0, "the last hit is the note's own level: {swelling:?}");
        assert!(gains("R03").iter().all(|g| *g == 0.0));
    }

    #[test]
    fn a_retrigger_dynamics_start_from_the_notes_volume() {
        let mut column = with_commands(2, &[(0, "C-4")], &[(0, "R76")]);
        column.vol = vec![Some("64".into()), None];
        let events = played(2, vec![column], Mode::Song);
        let gain = |i: usize| events[i]["args"]["gain"].as_str().unwrap().trim_end_matches("db").parse::<f64>().unwrap();
        assert!((gain(0) - volume_db(64)).abs() < 0.01 && (gain(1) - volume_db(64) + 20.0 * 2.0_f64.log10()).abs() < 0.01, "{:?}", (gain(0), gain(1)));
    }

    #[test]
    fn a_volume_slide_moves_the_held_notes_level_during_its_row() {
        let mut column = column(6, &[(0, "A-4")]);
        column.vol = vec![Some("128".into()), None, None, None, None, None];
        column.fx = vec![vec![None, None, Some("O40".into()), None, Some("I20".into()), None]];
        let events = played(6, vec![column], Mode::Song);
        let sets: Vec<_> = events.iter().filter(|e| e["op"] == "set").collect();
        assert_eq!(sets.len(), 2, "{sets:?}");
        assert_eq!(sets[0]["at"].as_u64(), Some(2 * 6000));
        assert_eq!(sets[0]["args"]["gain"].as_str(), Some(format!("{:.3}db", volume_db(64)).as_str()));
        assert_eq!(sets[1]["args"]["gain"].as_str(), Some(format!("{:.3}db", volume_db(96)).as_str()));
        assert_eq!(sets[0]["glide"], "0.125000sec");
    }

    #[test]
    fn a_length_releases_the_note_part_way_through_its_row() {
        let mut shortened = column(4, &[(0, "C-4"), (2, "E-4")]);
        shortened.fx = vec![vec![None, None, Some("C06".into()), None]];
        let held: Vec<_> = played(4, vec![shortened], Mode::Song).iter().filter(|e| e["op"] == "note").map(duration).collect();
        assert!((held[0] - 0.25).abs() < 1e-5 && (held[1] - 0.0625).abs() < 1e-5, "{held:?}");
        let mut bad = column(2, &[(0, "C-4")]);
        bad.fx = vec![vec![Some("C10".into()), None]];
        assert!(bad.validate(2).is_err());
    }

    fn chance_column(rows: usize, cells: &[(usize, &str)], chances: &[(usize, &str)]) -> NoteColumn {
        let mut column = column(rows, cells);
        column.fx = vec![vec![None; rows]];
        for (row, text) in chances {
            column.fx[0][*row] = Some((*text).into());
        }
        column
    }

    #[test]
    fn a_note_that_does_not_play_leaves_the_one_held_before_it_sounding() {
        let notes = |chance: &str| -> Vec<Value> {
            played(4, vec![chance_column(4, &[(0, "C-4"), (2, "E-4")], &[(2, chance)])], Mode::Song).into_iter().filter(|e| e["op"] == "note").collect()
        };
        let sure = notes("YFF");
        assert_eq!(sure.len(), 2);
        assert!((duration(&sure[0]) - 0.25).abs() < 1e-5, "E-4 ends C-4 at its row: {}", duration(&sure[0]));
        let never = notes("Y01");
        assert_eq!(never.len(), 1, "E-4 does not play");
        assert!((duration(&never[0]) - 0.5).abs() < 1e-5, "C-4 rings on to the end of the pattern: {}", duration(&never[0]));
    }

    #[test]
    fn chance_is_rolled_afresh_and_a_note_and_its_hold_agree() {
        // 64 notes at half chance: some play and some do not
        let cells: Vec<_> = (0..64).map(|row| (row, "C-4")).collect();
        let chances: Vec<_> = (0..64).map(|row| (row, "Y80")).collect();
        let events = played(64, vec![chance_column(64, &cells, &chances)], Mode::Song);
        let notes: Vec<_> = events.iter().filter(|e| e["op"] == "note").collect();
        assert!((10..54).contains(&notes.len()), "{}", notes.len());
        // each note that played is held to the next one that played, never to one that was skipped
        for pair in notes.windows(2) {
            let gap = (pair[1]["at"].as_u64().unwrap() - pair[0]["at"].as_u64().unwrap()) as f64 / 48000.0;
            assert!((duration(pair[0]) - gap).abs() < 1e-4, "{} against {gap}", duration(pair[0]));
        }
        // a note with no chance written always plays
        assert_eq!(played(4, vec![chance_column(4, &[(0, "C-4"), (1, "C-4")], &[])], Mode::Song).iter().filter(|e| e["op"] == "note").count(), 2);
    }

    /// Where the notes of a one-column pattern start, in samples, with `swing` percent on the song
    /// and `own` on the pattern. A row is 6000 samples.
    fn starts(rows: usize, swing: f64, own: Option<f64>, ramp: bool) -> Vec<u64> {
        let cells: Vec<_> = (0..rows).map(|row| (row, "C-4")).collect();
        played_with(rows, vec![column(rows, &cells)], Mode::Song, |play| {
            play.song.swing = swing;
            play.song.patterns[0].swing = own;
            if ramp {
                play.song.patterns[0].timing = Some(Timing { bpm: 120.0, rows_per_beat: 4, beats_per_bar: 4, beat_unit: 4, tempo: vec![EnvelopePoint { row: 0.0, value: 60.0 }, EnvelopePoint { row: rows as f64, value: 180.0 }] });
            }
        })
        .iter()
        .filter(|e| e["op"] == "note")
        .map(|e| e["at"].as_u64().unwrap())
        .collect()
    }

    #[test]
    fn swing_moves_time_from_the_second_row_of_each_pair_to_the_first() {
        assert_eq!(starts(4, 0.0, None, false), [0, 6000, 12000, 18000]);
        // half the shorter row moves: 3 parts to 1
        assert_eq!(starts(4, 50.0, None, false), [0, 9000, 12000, 21000]);
        // a pattern of its own says straight even when the song swings, and the reverse
        assert_eq!(starts(4, 50.0, Some(0.0), false), [0, 6000, 12000, 18000]);
        assert_eq!(starts(4, 0.0, Some(50.0), false), [0, 9000, 12000, 21000]);
        // an odd last row has no partner, so it starts where the pairs before it end
        assert_eq!(starts(3, 50.0, None, false), [0, 9000, 12000]);
    }

    #[test]
    fn swing_keeps_the_length_of_each_pair_through_a_tempo_ramp() {
        let straight = starts(6, 0.0, None, true);
        let swung = starts(6, 50.0, None, true);
        for pair in [0, 2, 4] {
            assert_eq!(swung[pair], straight[pair], "a pair starts where it did: {swung:?} against {straight:?}");
        }
        assert!(swung[1] > straight[1] && swung[3] > straight[3], "the second row of a pair starts later: {swung:?} against {straight:?}");
    }

    #[test]
    fn swing_has_to_be_between_0_and_75() {
        let mut song = song();
        song.swing = 75.0;
        song.validate().unwrap();
        song.swing = 76.0;
        assert!(song.validate().is_err());
        song.swing = 0.0;
        song.patterns[0].swing = Some(-1.0);
        assert!(song.validate().is_err());
    }

    #[test]
    fn offset_and_reverse_reach_only_instruments_that_have_the_parameter() {
        let cells = [(0, "C-4"), (2, "C-4")];
        let commands = [(0, "S40"), (2, "B00")];
        let notes = |parameters: &[&str]| {
            played_with(4, vec![with_commands(4, &cells, &commands)], Mode::Song, |play| {
                play.instrument_parameters.insert("tone".into(), parameters.iter().map(|p| parameter(p)).collect());
            })
        };
        let events = notes(&["begin", "rate"]);
        assert_eq!(events[0]["args"]["begin"], 0.25);
        assert_eq!(events[1]["args"]["rate"], -1.0);
        let events = notes(&[]);
        assert!(events[0]["args"].get("begin").is_none() && events[1]["args"].get("rate").is_none());
    }

    #[test]
    fn a_sliced_sample_plays_the_piece_for_each_key_from_c4() {
        let events = played_with(4, vec![column(4, &[(0, "C-4"), (1, "D#4"), (2, "C-3"), (3, "E-4")])], Mode::Song, |play| {
            play.instrument_slices.insert("tone".into(), 4);
        });
        let pieces: Vec<_> = events.iter().map(|e| e["args"]["sample"].as_u64().unwrap()).collect();
        assert_eq!(pieces, [0, 3], "C-3 is below the first piece and E-4 is past the last");
        assert_eq!(events[0]["args"]["freq"], "c4");
        assert_eq!(note_args("C-4", None).unwrap().unwrap()["freq"], "c4");
    }

    #[test]
    fn a_glide_carries_the_held_note_to_the_new_pitch_instead_of_starting_one() {
        // C-4 at row 0, glide to A-4 over one row (nine semitones at 12 sixteenths a tick is one row) at row 2, an OFF at row 5
        let column = with_commands(8, &[(0, "C-4"), (2, "A-4"), (5, "OFF")], &[(2, "G0C")]);
        let events = played(8, vec![column], Mode::Song);
        let notes: Vec<_> = events.iter().filter(|e| e["op"] == "note").collect();
        assert_eq!(notes.len(), 1, "the glide starts no note of its own");
        assert!((duration(notes[0]) - 5.0 * 0.125).abs() < 1e-5, "the first note is held to the OFF");
        let set = events.iter().find(|e| e["op"] == "set").expect("the glide");
        assert_eq!(set["id"], notes[0]["id"]);
        assert_eq!(set["at"], 2 * 6000);
        assert_eq!(set["args"]["freq"], "440.000hz");
        assert_eq!(set["glide"], "0.125000sec");
        // with nothing held, a glide note is just a note
        let column = with_commands(4, &[(1, "A-4")], &[(1, "G0C")]);
        assert_eq!(played(4, vec![column], Mode::Song).iter().filter(|e| e["op"] == "note").count(), 1);
        // and a glide to another instrument's note cannot carry the sound on
        let mut column = with_commands(8, &[(0, "C-4"), (2, "A-4")], &[(2, "G0C")]);
        column.inst[2] = Some("alt".into());
        assert_eq!(played(8, vec![column], Mode::Song).iter().filter(|e| e["op"] == "note").count(), 2);
    }

    #[test]
    fn a_slide_bends_the_held_note_during_its_row() {
        // U10 a row below the note takes it up an octave; D10 on the row after brings it back
        let column = with_commands(8, &[(0, "A-4")], &[(2, "U10"), (3, "D10")]);
        let events = played(8, vec![column], Mode::Song);
        let sets: Vec<_> = events.iter().filter(|e| e["op"] == "set").collect();
        assert_eq!(sets.len(), 2);
        assert_eq!((sets[0]["at"].as_u64(), sets[0]["args"]["freq"].as_str()), (Some(2 * 6000), Some("880.000hz")));
        assert_eq!((sets[1]["at"].as_u64(), sets[1]["args"]["freq"].as_str()), (Some(3 * 6000), Some("440.000hz")));
        assert_eq!(sets[0]["glide"], "0.125000sec");
    }

    fn lane_on(play: &mut Playback, cutoff: &[(usize, &str)]) {
        play.instrument_parameters.insert("tone".into(), vec![parameter("cutoff")]);
        let lane = &mut play.song.patterns[0].fx[0][0];
        lane.target = Some(crate::commands::FxTarget::Instrument { instrument: "tone".into(), parameter: "cutoff".into() });
        let values: Vec<_> = cutoff.iter().map(|(row, value)| (*row, value.parse().unwrap())).collect();
        lane.held(&values);
    }

    #[test]
    fn track_commands_set_the_volume_pan_and_effect_parameters_of_their_strip() {
        use crate::commands::FxTarget;
        assert_eq!(TrackCommand::parse("L80"), Some(TrackCommand::Volume(0x80)));
        assert_eq!(TrackCommand::parse("PFF"), Some(TrackCommand::Pan(0xFF)));
        assert_eq!(TrackCommand::parse("13FF"), Some(TrackCommand::Device { device: 1, parameter: 3, value: 0xFF }));
        assert_eq!(TrackCommand::parse("02FF"), Some(TrackCommand::Macro { index: 2, value: 0xFF }));
        for bad in ["", "L8", "L800", "Q80", "l80", "0000", "1000", "12G0", "12F", "12FFF"] {
            assert_eq!(TrackCommand::parse(bad), None, "{bad}");
        }
        let strip = Track { fx: vec![Device { id: "d0".into(), definition: "filter".into(), ..Device::default() }], ..Track::default() };
        let parameters: crate::commands::EffectParameters = [("filter".into(), vec![crate::commands::Parameter { name: "cutoff".into(), min: 20.0, max: 20000.0, default: 2400.0, unit: Some("hz".into()), ..Default::default() }])].into();
        let aim = |text: &str| TrackCommand::parse(text).and_then(|c| c.target(&strip, &parameters));
        assert_eq!(aim("L00"), Some((FxTarget::Volume, -60.0)));
        let (target, db) = aim("LFF").unwrap();
        assert_eq!((target, db), (FxTarget::Volume, 3.0));
        assert!((aim("LB4").unwrap().1).abs() < 0.1, "about 0 dB at B4");
        assert_eq!(aim("P00"), Some((FxTarget::Pan, -1.0)));
        assert_eq!(aim("P80"), Some((FxTarget::Pan, 0.0)));
        assert_eq!(aim("PFF"), Some((FxTarget::Pan, 1.0)));
        assert_eq!(aim("1100"), Some((FxTarget::Effect { device: "d0".into(), parameter: "cutoff".into() }, 20.0)));
        assert_eq!(aim("11FF").unwrap().1, 20000.0);
        assert_eq!(aim("2100"), None, "no second effect");
        assert_eq!(aim("1200"), None, "no second parameter");
        let knobs = Track { macros: vec![Macro { name: "Open".into(), value: 0.0, links: vec![] }], ..strip.clone() };
        let aim_macro = |text: &str| TrackCommand::parse(text).and_then(|c| c.target(&knobs, &parameters));
        assert_eq!(aim_macro("0100"), Some((crate::commands::FxTarget::Macro { index: 0 }, 0.0)));
        assert_eq!(aim_macro("01FF").unwrap().1, 1.0);
        assert_eq!(TrackCommand::parse("0F80"), Some(TrackCommand::Macro { index: 15, value: 0x80 }), "a macro is named by one hex digit, up to F");
        assert!(!aim_macro("0200").unwrap().0.available(&knobs, &parameters), "no second macro: the command is skipped");
        // a track's commands, in the fx columns of its note columns, reach the engine as one routing edit at the start of their row
        let mut with_commands = column(4, &[]);
        with_commands.fx = vec![vec![None, Some("L00".into()), None, Some("P00".into())]];
        let events = played_with(4, vec![with_commands], Mode::Song, |_| {});
        let edits: Vec<_> = events.iter().filter(|e| e["op"] == "routing").collect();
        assert_eq!(edits.len(), 2, "{edits:?}");
        assert_eq!((edits[0]["at"].as_u64(), edits[0]["edits"][0]["args"]["level"].as_f64()), (Some(6000), Some(10.0_f64.powf(-3.0))));
        assert_eq!((edits[1]["at"].as_u64(), edits[1]["edits"][0]["args"]["balance"].as_f64()), (Some(18000), Some(-1.0)));
        // a track command does not disturb the note commands of the same cells
        let mut mixed = with_commands_and_note();
        assert_eq!(mixed.commands(0).len(), 1);
        assert!(mixed.validate(4).is_ok());
        mixed.fx[0][2] = Some("Q80".into());
        assert!(mixed.validate(4).is_err());
        // a song with a track command the engine cannot read, or the wrong number of strips, is refused
        let mut bad = song();
        bad.patterns[0].commands = vec![vec![vec![Some("Q80".into()), None]]; bad.returns.len() + 1];
        assert!(bad.validate().is_err());
        bad.patterns[0].commands = vec![vec![]; bad.returns.len() + 2];
        assert!(bad.validate().is_err());
    }

    fn with_commands_and_note() -> NoteColumn {
        let mut column = column(4, &[(0, "C-4")]);
        column.fx = vec![vec![Some("S80".into()), Some("L80".into()), Some("12FF".into()), None]];
        column
    }

    #[test]
    fn macro_automation_ramps_every_link_at_the_same_sample() {
        let events = played_with(2, vec![column(2, &[])], Mode::Song, |play| {
            let track = &mut play.song.tracks[0];
            track.fx = ["a", "b"].map(|id| Device { id: id.into(), definition: "level".into(), ..Default::default() }).into();
            track.macros = vec![Macro { name: "Crossfade".into(), value: 0.0, links: vec![
                MacroLink { device: "a".into(), parameter: "amount".into(), min: 0.0, max: 1.0 },
                MacroLink { device: "b".into(), parameter: "amount".into(), min: 1.0, max: 0.0 },
            ] }];
            play.parameters.insert("level".into(), vec![parameter("amount")]);
            let lane = &mut play.song.patterns[0].fx[0][0];
            lane.target = Some(crate::commands::FxTarget::Macro { index: 0 });
            lane.envelope = Some(Envelope { enabled: true, mode: "linear".into(), points: vec![
                EnvelopePoint { row: 0.0, value: 0.0 }, EnvelopePoint { row: 2.0, value: 1.0 },
            ] });
        });
        let at = |sample| events.iter().find(|event| event["op"] == "routing" && event["at"] == sample).unwrap()["edits"].as_array().unwrap();
        let midpoint = at(6000);
        assert_eq!(midpoint.len(), 2);
        assert!(midpoint.iter().all(|edit| edit["args"]["amount"] == 0.5));
        let ramps = at(6001);
        assert_eq!(ramps.len(), 2);
        assert_eq!(ramps[0]["args"]["amount"], 1.0);
        assert_eq!(ramps[1]["args"]["amount"], 0.0);
        assert!(ramps.iter().all(|edit| edit["glide"] == json!(5999.0 / 48000.0)));
    }

    #[test]
    fn an_instrument_lane_changes_the_notes_sounding_as_it_passes() {
        // C-4 held through row 4's OFF, another note from row 6; the lane sets a value on rows 3, 5 and 7
        let events = played_with(10, vec![column(10, &[(0, "C-4"), (4, "OFF"), (6, "E-4")])], Mode::Song, |play| lane_on(play, &[(3, "0.5"), (5, "0.25"), (7, "0.75")]));
        let notes: Vec<_> = events.iter().filter(|e| e["op"] == "note").collect();
        let sets: Vec<_> = events.iter().filter(|e| e["op"] == "set").collect();
        assert_eq!(notes.len(), 2);
        assert!(notes[0]["id"].is_u64() && notes[1]["id"].is_u64(), "notes that a lane can change are named");
        // the held note hears row 3 only: it has been released by row 5; the later note hears row 7 on (the envelope holds, and says so each row) and had row 5's value from the start
        let of = |note: &Value| sets.iter().filter(|e| e["id"] == note["id"]).collect::<Vec<_>>();
        let (first, second) = (of(notes[0]), of(notes[1]));
        assert_eq!(first.len(), 1, "{sets:?}");
        assert_eq!((first[0]["at"].as_u64(), first[0]["args"]["cutoff"].clone()), (Some(3 * 6000), json!(0.5)));
        assert_eq!(second[0]["at"].as_u64(), Some(7 * 6000));
        assert!(second.iter().all(|e| e["args"]["cutoff"] == json!(0.75)), "{second:?}");
        assert_eq!(notes[1]["args"]["cutoff"], json!(0.25));
        assert!(notes[0]["args"].get("cutoff").is_none());
    }

    #[test]
    fn an_envelope_on_an_instrument_lane_ramps_the_sounding_note() {
        let events = played_with(8, vec![column(8, &[(0, "C-4")])], Mode::Song, |play| {
            lane_on(play, &[]);
            play.song.patterns[0].fx[0][0].envelope = Some(Envelope {
                enabled: true,
                mode: "linear".into(),
                points: vec![EnvelopePoint { row: 0.0, value: 0.0 }, EnvelopePoint { row: 4.0, value: 1.0 }],
            });
        });
        let sets: Vec<_> = events.iter().filter(|e| e["op"] == "set").collect();
        // each of the four rows steps to its start value then glides to its end; after the last point nothing is sent... except it holds
        let row = |n: usize| sets.iter().filter(|e| e["at"].as_u64().is_some_and(|at| at / 6000 == n as u64)).collect::<Vec<_>>();
        assert_eq!(row(2)[0]["args"]["cutoff"], json!(0.5));
        assert_eq!(row(2)[1]["args"]["cutoff"], json!(0.75));
        assert_eq!(row(2)[1]["at"].as_u64(), Some(2 * 6000 + 1));
        assert!(row(2)[1]["glide"].as_str().unwrap().starts_with("0.12497"), "{:?}", row(2)[1]);
    }

    #[test]
    fn an_instrument_set_on_a_row_plays_the_notes_below_it_until_another_is_set() {
        let mut notes = column(4, &[(0, "C-4"), (1, "D-4"), (2, "E-4"), (3, "F-4")]);
        notes.inst = vec![Some("tone".into()), None, Some("alt".into()), None];
        let targets: Vec<_> = played(4, vec![notes], Mode::Song)
            .iter()
            .map(|e| e["target"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(targets, ["voice", "voice", "other", "other"]);
        let mut silent = column(4, &[(1, "C-4")]);
        silent.inst = vec![None; 4];
        assert!(played(4, vec![silent], Mode::Song).is_empty(), "no instrument set, so nothing plays");
    }

    #[test]
    fn a_parameter_lane_reaches_only_the_notes_of_its_own_instrument() {
        let mut song = song();
        song.bpm = 120.0;
        song.tracks.truncate(1);
        let mut lane = FxLane::empty();
        lane.target = Some(crate::commands::FxTarget::Instrument {
            instrument: "tone".into(),
            parameter: "cutoff".into(),
        });
        lane.held(&[(1, 500.0)]);
        let mut notes = column(4, &[(0, "C-4"), (2, "D-4"), (3, "E-4")]);
        notes.inst = vec![Some("tone".into()), None, Some("alt".into()), Some("tone".into())];
        song.patterns = vec![Pattern {
            timing: None,
            swing: None,
            id: "a".into(),
            name: "a".into(),
            rows: 4,
            notes: vec![vec![notes]],
            fx: vec![vec![lane]],
            return_fx: vec![],
            master_fx: vec![FxLane::empty()],
            commands: vec![],
        }];
        song.sequence = vec!["a".into()];
        let mut play = Playback::new(0, song, Mode::Song, "a".into(), 0).unwrap();
        let cutoff = crate::commands::Parameter { name: "cutoff".into(), min: 20.0, max: 20000.0, default: 2400.0, unit: Some("hz".into()), ..Default::default() };
        play.instrument_parameters = BTreeMap::from([("tone".into(), vec![cutoff.clone()]), ("alt".into(), vec![cutoff])]);
        let targets = BTreeMap::from([("tone".into(), "voice".into()), ("alt".into(), "other".into())]);
        let (events, _) = play.events_until(0, play.row_at(4, 48000.0), 48000.0, &targets).unwrap();
        let notes: Vec<_> = events.iter().filter(|e| e["op"] == "note").collect();
        let cutoffs: Vec<_> = notes.iter().map(|e| e["args"].get("cutoff").and_then(|c| c.as_str()).unwrap_or("-").to_string()).collect();
        // the lane starts at row 1: the first tone note is before it, the alt note is another instrument's, the last is after it
        assert_eq!(cutoffs, ["-", "-", "500hz"]);
        // the first note is still sounding at row 1 and is changed then; the alt note's instrument has no lane
        let sets: Vec<_> = events.iter().filter(|e| e["op"] == "set").collect();
        assert_eq!(sets.len(), 1, "{sets:?}");
        assert_eq!((sets[0]["id"].clone(), sets[0]["at"].as_u64(), sets[0]["args"]["cutoff"].clone()), (notes[0]["id"].clone(), Some(6000), json!("500hz")));
    }

    #[test]
    fn instrument_parameters_start_each_note_from_the_base_value_then_the_lane() {
        let mut song = song();
        song.bpm = 120.0;
        song.tracks.truncate(1);
        let mut lane = FxLane::empty();
        lane.target = Some(crate::commands::FxTarget::Instrument {
            instrument: "tone".into(),
            parameter: "cutoff".into(),
        });
        lane.held(&[(2, 500.0)]);
        song.patterns = vec![Pattern {
            timing: None,
            swing: None,
            id: "a".into(),
            name: "a".into(),
            rows: 4,
            notes: vec![vec![column(4, &[(0, "C-4"), (2, "D-4"), (3, "E-4")])]],
            fx: vec![vec![lane]],
            return_fx: vec![],
            master_fx: vec![FxLane::empty()],
            commands: vec![],
        }];
        song.sequence = vec!["a".into()];
        song.instrument_values = BTreeMap::from([(
            "tone".into(),
            BTreeMap::from([("cutoff".into(), 2000.0), ("ignored".into(), 1.0)]),
        )]);
        let mut play = Playback::new(0, song, Mode::Song, "a".into(), 0).unwrap();
        play.instrument_parameters = BTreeMap::from([(
            "tone".into(),
            vec![crate::commands::Parameter {
                name: "cutoff".into(),
                min: 20.0,
                max: 20000.0,
                default: 2400.0,
                unit: Some("hz".into()),
                ..Default::default()
            }],
        )]);
        let targets = BTreeMap::from([("tone".into(), "voice".into())]);
        let (events, _) = play
            .events_until(0, play.row_at(4, 48000.0), 48000.0, &targets)
            .unwrap();
        let cutoffs: Vec<_> = events
            .iter()
            .map(|e| e["args"]["cutoff"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(cutoffs, ["2000hz", "500hz", "500hz"]);
        assert!(events.iter().all(|e| e["args"].get("ignored").is_none()));
    }

    #[test]
    fn note_columns_are_validated() {
        let mut song = song();
        song.patterns[0].notes[0][0].vol = vec![Some("129".into()), None];
        assert!(song.validate().is_err());
        song.patterns[0].notes[0][0].vol.clear();
        song.patterns[0].notes[0][0].delay = vec![Some("256".into()), None];
        assert!(song.validate().is_err());
        song.patterns[0].notes[0][0].delay.clear();
        song.patterns[0].notes[0][0].notes[0] = Some("OFF".into());
        song.validate().unwrap();
        song.patterns[0].notes[0] = vec![NoteColumn::blank(2); 9];
        assert!(song.validate().is_err());
        song.patterns[0].notes[0].clear();
        assert!(song.validate().is_err());
    }

    #[test]
    fn pattern_loops_independently_and_song_can_start_at_a_selected_entry() {
        let mut play = Playback::new(0, song(), Mode::Pattern, "b".into(), 0).unwrap();
        assert_eq!(play.location(100).unwrap().row, 1);
        assert!(!play.finished(1_000_000, 48000.0));
        assert_eq!(
            play.row_at(100_000, 48000.0),
            (100_000.0_f64 * 48000.0 * 60.0 / 137.0 / 4.0).round() as u64
        );
        let (events, skipped) = play
            .events_until(
                play.row_at(5, 48000.0),
                play.row_at(6, 48000.0),
                48000.0,
                &BTreeMap::from([("tone".into(), "voice".into())]),
            )
            .unwrap();
        assert_eq!(skipped, 5);
        assert_eq!(events.len(), 1);
        let play = Playback::new(0, song(), Mode::Song, "a".into(), 1).unwrap();
        assert_eq!(play.location(0).unwrap().pattern.id, "b");
        assert_eq!(play.location(3).unwrap().sequence_position, Some(2));
        assert!(play.location(5).is_none());
    }
}
