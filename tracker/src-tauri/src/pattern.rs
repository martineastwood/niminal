use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

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
    pub values: Vec<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<Envelope>,
}
impl FxLane {
    #[cfg(test)]
    pub fn empty(rows: usize) -> Self {
        Self {
            target: Some(crate::commands::FxTarget::Volume),
            values: vec![None; rows],
            envelope: None,
        }
    }
}

pub const MAX_NOTE_COLUMNS: usize = 8;

/// One note column of a track. Every sub-column holds a cell per row; the optional ones
/// may be left empty, which reads as blank everywhere.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct NoteColumn {
    /// A note such as `C-4`, or `OFF` to end the note that is sounding.
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
        {
            return Err("Every note sub-column needs a cell for each row".into());
        }
        for note in self.notes.iter().flatten() {
            if note != "OFF" {
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
    /// The delay as a fraction of a row, from 0 up to just under 1.
    pub fn delayed(&self, row: usize) -> f64 {
        f64::from(
            Self::text(&self.delay, row)
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(0),
        ) / 256.0
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Pattern {
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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Song {
    pub bpm: f64,
    pub rows_per_beat: u32,
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
            if pattern.fx.len() != self.tracks.len() || pattern.return_fx.len() != self.returns.len() {
                return Err("Patterns need parameter lanes for every track and return, and the master".into());
            }
            let groups = || pattern.fx.iter().chain(&pattern.return_fx).chain([&pattern.master_fx]);
            if groups().any(|lanes| {
                lanes.is_empty()
                    || lanes.len() > 4
                    || lanes.iter().any(|lane| lane.values.len() < pattern.rows || lane.values.len() > 256)
            }) {
                return Err("Patterns need 1–4 parameter lanes per track, return and the master".into());
            }
            if pattern.return_fx.iter().flatten().chain(&pattern.master_fx).any(|lane| matches!(lane.target, Some(crate::commands::FxTarget::Instrument { .. }))) {
                return Err("Returns and the master have no instruments to automate".into());
            }
            for lane in groups().flatten() {
                if lane.envelope.as_ref().is_some_and(|e| !e.valid()) {
                    return Err("Invalid automation envelope".into());
                }
                if lane.target.is_none()
                    && (lane.values.iter().any(Option::is_some)
                        || lane.envelope.as_ref().is_some_and(|e| !e.points.is_empty()))
                {
                    return Err("Choose a parameter for every populated lane".into());
                }
                if lane
                    .values
                    .iter()
                    .flatten()
                    .any(|v| !v.parse::<f64>().is_ok_and(|n| n.is_finite()))
                {
                    return Err("Parameter lanes need finite numeric values".into());
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
            && self.bpm == other.bpm
            && self.rows_per_beat == other.rows_per_beat
            && self.sequence == other.sequence
            && self.patterns.iter().all(|p| {
                other
                    .patterns
                    .iter()
                    .any(|q| p.id == q.id && p.rows == q.rows)
            })
    }
}

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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Pattern,
    Song,
}

pub struct Playback {
    pub origin: u64,
    pub next_row: u64,
    pub parameters: crate::commands::EffectParameters,
    pub instrument_parameters: crate::commands::EffectParameters,
    /// Each track's instrument parameter lanes, by instrument and parameter, as of the last row played.
    lane_state: Vec<BTreeMap<(String, String), f64>>,
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
        Ok(Self {
            origin,
            next_row: 0,
            parameters: BTreeMap::new(),
            instrument_parameters: BTreeMap::new(),
            lane_state: Vec::new(),
            song,
            mode,
            pattern_id,
            start_position,
        })
    }

    pub fn row_at(&self, index: u64, sample_rate: f64) -> u64 {
        self.origin + (index as f64 * self.song.row_samples(sample_rate)).round() as u64
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
            let index = ((sample - self.origin) as f64 / self.song.row_samples(sample_rate)).floor() as u64;
            let now = self
                .location(index)
                .and_then(|p| Some((p.sequence_position?, p.row as u64)));
            if let Some((position, row)) = now {
                let start = index - row;
                self.origin = self.row_at(start, sample_rate);
                self.start_position = position;
                self.next_row = self.next_row.saturating_sub(start);
            }
        }
        self.song.loop_range = range;
    }

    pub fn position(&self, sample: u64, sample_rate: f64) -> Option<Position<'_>> {
        if sample < self.origin {
            return None;
        }
        let index =
            ((sample - self.origin) as f64 / self.song.row_samples(sample_rate)).floor() as u64;
        self.location(index)
    }

    /// How far through the current row `sample` is, from 0 up to 1.
    pub fn phase(&self, sample: u64, sample_rate: f64) -> Option<f64> {
        if sample < self.origin {
            return None;
        }
        Some(((sample - self.origin) as f64 / self.song.row_samples(sample_rate)).fract())
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

    /// How many rows a note started on row `index` (itself `delay` rows late) is held: until
    /// the column next has a note or an OFF, or until the song or pattern runs out.
    fn hold_rows(&self, index: u64, track: usize, column: usize, delay: f64) -> f64 {
        const LOOKAHEAD: u64 = 16384;
        for ahead in 1..=LOOKAHEAD {
            let Some(position) = self.location(index + ahead) else {
                // The song ends here.
                return (ahead as f64 - delay).max(MIN_HOLD);
            };
            let Some(pattern) = self.track_pattern(&position, track) else {
                // The track is muted from here, so the note ends where the mute begins.
                return (ahead as f64 - delay).max(MIN_HOLD);
            };
            let Some(next) = pattern.notes[track].get(column) else {
                continue;
            };
            if next.note(position.row).is_some() {
                return (ahead as f64 + next.delayed(position.row) - delay).max(MIN_HOLD);
            }
        }
        (LOOKAHEAD as f64).max(MIN_HOLD)
    }

    /// The engine edits for one parameter lane at one row of a pattern: a value or the steps of an
    /// envelope across the row, for the strip named `slot` (a track's number, `r<n>` or `m`).
    /// `when` is the pattern's rows, the row and the sample it starts at; `out` takes the edits made at
    /// the start of the row and the timed events for the rest.
    fn lane_events(
        &self,
        slot: &str,
        strip: &Track,
        lane: &FxLane,
        when: (usize, usize, u64),
        sample_rate: f64,
        out: (&mut Vec<Value>, &mut Vec<Value>),
    ) -> Result<(), String> {
        let (rows, row, at) = when;
        let (edits, events) = out;
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
                | crate::commands::FxTarget::Instrument { .. } => false,
            };
            for pair in breaks.windows(2) {
                let Some(value) = envelope.value(pair[0], db) else {
                    continue;
                };
                let point_at = at
                    + ((pair[0] - start) * self.song.row_samples(sample_rate)).round()
                        as u64;
                let end_at = at
                    + ((pair[1] - start) * self.song.row_samples(sample_rate)).round()
                        as u64;
                let mut point_edits: Vec<_> =
                    target.unmap(slot, strip).into_iter().collect();
                point_edits.push(crate::commands::edit(
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
                    ramp["glide"] = json!((end_at - point_at - 1) as f64 / sample_rate);
                    events.push(json!({"op":"routing", "at":point_at+1, "tag":"tracker-pattern", "edits":[ramp]}));
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
                events.push(json!({"op":"routing", "at":self.row_at(self.next_row+1, sample_rate), "tag":"tracker-pattern", "edits":[edit]}));
            }
            return Ok(());
        }
        let (Some(target), Some(Some(value))) = (&lane.target, lane.values.get(row))
        else {
            return Ok(());
        };
        let edit =
            crate::commands::edit(target, value, slot, strip, &self.parameters)?;
        edits.extend(target.unmap(slot, strip));
        edits.push(edit);
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
                            _ => lane
                                .values
                                .get(position.row)
                                .and_then(|v| v.as_deref())
                                .and_then(|v| v.parse().ok()),
                        };
                        if let Some(value) = value {
                            state[index].insert((instrument.clone(), parameter.clone()), value);
                        }
                        continue;
                    }
                    self.lane_events(
                        &index.to_string(),
                        track,
                        lane,
                        (pattern.rows, position.row, at),
                        sample_rate,
                        (&mut edits, &mut events),
                    )?;
                }
                if !edits.is_empty() {
                    events.push(
                        json!({"op":"routing", "at":at, "tag":"tracker-pattern", "edits":edits}),
                    );
                }
                for (column_index, column) in pattern.notes[index].iter().enumerate() {
                    let Some(note) = column.note(position.row) else {
                        continue;
                    };
                    if note == "OFF" {
                        // It ends the note before it; that note was timed to stop here.
                        continue;
                    }
                    // A note with no instrument set at or above it is silent, not an error.
                    let Some(id) = column.instrument(position.row) else {
                        continue;
                    };
                    let target = targets
                        .get(id)
                        .ok_or_else(|| format!("Apply the instrument played on {} first", track.name))?;
                    let row_seconds = 60.0 / self.song.bpm / f64::from(self.song.rows_per_beat);
                    let delay = column.delayed(position.row);
                    let hold = self.hold_rows(self.next_row, index, column_index, delay);
                    let mut args = json!({"freq":pitch(note)?});
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
                    events.push(json!({"op":"note", "at":at + (delay * self.song.row_samples(sample_rate)).round() as u64, "target":target, "to":format!("in_{index}"), "tag":"tracker-pattern",
                        "dur":format!("{:.6}sec", hold * row_seconds), "args":args}));
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
            for (slot, strip, lanes) in strips {
                for lane in lanes {
                    self.lane_events(&slot, strip, lane, (pattern.rows, position.row, at), sample_rate, (&mut edits, &mut events))?;
                }
            }
            if !edits.is_empty() {
                events.push(json!({"op":"routing", "at":at, "tag":"tracker-pattern", "edits":edits}));
            }
            self.next_row += 1;
        }
        self.lane_state = state;
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
            tracks: (0..4)
                .map(|i| Track {
                    name: format!("Track {i}"),
                    ..Track::default()
                })
                .collect(),
            patterns: [("a", 2, "C-4"), ("b", 3, "D#4")]
                .into_iter()
                .map(|(id, rows, note)| Pattern {
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
                    fx: (0..4).map(|_| vec![FxLane::empty(rows)]).collect(),
                    return_fx: vec![],
                    master_fx: vec![FxLane::empty(rows)],
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
        let mut song = song();
        song.bpm = 120.0;
        song.tracks.truncate(1);
        song.patterns = vec![Pattern {
            id: "a".into(),
            name: "a".into(),
            rows,
            notes: vec![columns],
            fx: vec![vec![FxLane::empty(rows)]],
            return_fx: vec![],
            master_fx: vec![FxLane::empty(rows)],
        }];
        song.sequence = vec!["a".into()];
        let mut play = Playback::new(0, song, mode, "a".into(), 0).unwrap();
        let targets = BTreeMap::from([
            ("tone".into(), "voice".into()),
            ("alt".into(), "other".into()),
        ]);
        let horizon = play.row_at(rows as u64, 48000.0);
        play.events_until(0, horizon, 48000.0, &targets).unwrap().0
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
        let mut lane = FxLane::empty(4);
        lane.target = Some(crate::commands::FxTarget::Instrument {
            instrument: "tone".into(),
            parameter: "cutoff".into(),
        });
        lane.values[1] = Some("500".into());
        let mut notes = column(4, &[(0, "C-4"), (2, "D-4"), (3, "E-4")]);
        notes.inst = vec![Some("tone".into()), None, Some("alt".into()), Some("tone".into())];
        song.patterns = vec![Pattern {
            id: "a".into(),
            name: "a".into(),
            rows: 4,
            notes: vec![vec![notes]],
            fx: vec![vec![lane]],
            return_fx: vec![],
            master_fx: vec![FxLane::empty(4)],
        }];
        song.sequence = vec!["a".into()];
        let mut play = Playback::new(0, song, Mode::Song, "a".into(), 0).unwrap();
        let cutoff = crate::commands::Parameter { name: "cutoff".into(), min: 20.0, max: 20000.0, default: 2400.0, unit: Some("hz".into()) };
        play.instrument_parameters = BTreeMap::from([("tone".into(), vec![cutoff.clone()]), ("alt".into(), vec![cutoff])]);
        let targets = BTreeMap::from([("tone".into(), "voice".into()), ("alt".into(), "other".into())]);
        let (events, _) = play.events_until(0, play.row_at(4, 48000.0), 48000.0, &targets).unwrap();
        let cutoffs: Vec<_> = events.iter().map(|e| e["args"].get("cutoff").and_then(|c| c.as_str()).unwrap_or("-").to_string()).collect();
        // the lane starts at row 1: the first tone note is before it, the alt note is another instrument's, the last is after it
        assert_eq!(cutoffs, ["-", "-", "500hz"]);
    }

    #[test]
    fn instrument_parameters_start_each_note_from_the_base_value_then_the_lane() {
        let mut song = song();
        song.bpm = 120.0;
        song.tracks.truncate(1);
        let mut lane = FxLane::empty(4);
        lane.target = Some(crate::commands::FxTarget::Instrument {
            instrument: "tone".into(),
            parameter: "cutoff".into(),
        });
        lane.values[2] = Some("500".into());
        song.patterns = vec![Pattern {
            id: "a".into(),
            name: "a".into(),
            rows: 4,
            notes: vec![vec![column(4, &[(0, "C-4"), (2, "D-4"), (3, "E-4")])]],
            fx: vec![vec![lane]],
            return_fx: vec![],
            master_fx: vec![FxLane::empty(4)],
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
