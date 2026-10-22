use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Track {
    pub name: String,
    pub instrument: Option<String>,
    /// The track's effect chain, in signal order.
    #[serde(default)]
    pub fx: Vec<Device>,
    pub pan: f64,
    pub muted: bool,
    pub volume_db: f64,
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
}

impl Track {
    pub fn device(&self, id: &str) -> Option<&Device> {
        self.fx.iter().find(|device| device.id == id)
    }
    /// The devices that process audio: bypassed ones are left out of the engine graph.
    pub fn active(&self) -> impl Iterator<Item = &Device> {
        self.fx.iter().filter(|device| !device.bypass)
    }
}

pub const MAX_DEVICES: usize = 8;

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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Pattern {
    pub id: String,
    pub name: String,
    pub rows: usize,
    pub notes: Vec<Vec<Option<String>>>,
    pub fx: Vec<Vec<FxLane>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Song {
    pub bpm: f64,
    pub rows_per_beat: u32,
    pub tracks: Vec<Track>,
    pub patterns: Vec<Pattern>,
    pub sequence: Vec<String>,
}

impl Song {
    pub fn validate(&self) -> Result<(), String> {
        if !self.bpm.is_finite()
            || !(20.0..=300.0).contains(&self.bpm)
            || !(1..=16).contains(&self.rows_per_beat)
            || !(1..=16).contains(&self.tracks.len())
            || self.tracks.iter().any(|t| {
                !t.volume_db.is_finite()
                    || !(-60.0..=6.0).contains(&t.volume_db)
                    || !t.pan.is_finite()
                    || !(-1.0..=1.0).contains(&t.pan)
                    || t.fx.len() > MAX_DEVICES
                    || t.fx.iter().enumerate().any(|(i, device)| {
                        device.id.is_empty()
                            || device.id.len() > 40
                            || !device.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                            || t.fx[..i].iter().any(|other| other.id == device.id)
                            || device.values.values().any(|value| !value.is_finite())
                            || device.modulations.values().any(|lfo| !lfo.valid())
                    })
            })
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
                    .any(|notes| notes.len() < pattern.rows || notes.len() > 256)
            {
                return Err(
                    "Patterns need unique IDs, 1–256 rows and one note column per track".into(),
                );
            }
            if pattern.fx.len() != self.tracks.len()
                || pattern.fx.iter().any(|track| {
                    track.is_empty()
                        || track.len() > 4
                        || track.iter().any(|column| {
                            column.values.len() < pattern.rows || column.values.len() > 256
                        })
                })
            {
                return Err("Patterns need 1–4 FX columns per track".into());
            }
            for lane in pattern.fx.iter().flatten() {
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
            for note in pattern.notes.iter().flatten().flatten() {
                pitch(note)?;
            }
        }
        if self.sequence.iter().any(|id| !ids.contains(id)) {
            return Err("Sequence references a missing pattern".into());
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
        let mut row = index;
        for (sequence_position, id) in self
            .song
            .sequence
            .iter()
            .enumerate()
            .skip(self.start_position)
        {
            let pattern = self.song.patterns.iter().find(|p| p.id == *id)?;
            if row < pattern.rows as u64 {
                return Some(Position {
                    pattern,
                    row: row as usize,
                    sequence_position: Some(sequence_position),
                });
            }
            row -= pattern.rows as u64;
        }
        None
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
                let mut edits = Vec::new();
                for lane in &position.pattern.fx[index] {
                    if lane
                        .target
                        .as_ref()
                        .is_some_and(|target| !target.available(track, &self.parameters))
                    {
                        continue;
                    }
                    if let (Some(envelope), Some(target)) = (&lane.envelope, &lane.target)
                        && envelope.enabled
                    {
                        let start = position.row as f64;
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
                                .parameter(track, &self.parameters)
                                .is_some_and(|p| p.unit.as_deref() == Some("db")),
                            crate::commands::FxTarget::Pan => false,
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
                                target.unmap(index, track).into_iter().collect();
                            point_edits.push(crate::commands::edit(
                                target,
                                &value.to_string(),
                                index,
                                track,
                                &self.parameters,
                            )?);
                            events.push(json!({"op":"routing", "at":point_at, "tag":"tracker-pattern", "edits":point_edits}));
                            if envelope.mode == "linear" && end_at > point_at + 1 {
                                let value = envelope.value(pair[1], db).unwrap();
                                let mut ramp = crate::commands::edit(
                                    target,
                                    &value.to_string(),
                                    index,
                                    track,
                                    &self.parameters,
                                )?;
                                ramp["glide"] = json!((end_at - point_at - 1) as f64 / sample_rate);
                                events.push(json!({"op":"routing", "at":point_at+1, "tag":"tracker-pattern", "edits":[ramp]}));
                            }
                        }
                        if envelope.mode == "hold"
                            && position.row + 1 == position.pattern.rows
                            && let Some(point) = envelope
                                .points
                                .iter()
                                .find(|p| p.row == position.pattern.rows as f64)
                        {
                            let edit = crate::commands::edit(
                                target,
                                &point.value.to_string(),
                                index,
                                track,
                                &self.parameters,
                            )?;
                            events.push(json!({"op":"routing", "at":self.row_at(self.next_row+1, sample_rate), "tag":"tracker-pattern", "edits":[edit]}));
                        }
                        continue;
                    }
                    let (Some(target), Some(value)) = (&lane.target, &lane.values[position.row])
                    else {
                        continue;
                    };
                    let edit =
                        crate::commands::edit(target, value, index, track, &self.parameters)?;
                    edits.extend(target.unmap(index, track));
                    edits.push(edit);
                }
                if !edits.is_empty() {
                    events.push(
                        json!({"op":"routing", "at":at, "tag":"tracker-pattern", "edits":edits}),
                    );
                }
                if let (Some(id), Some(note)) = (
                    &track.instrument,
                    &position.pattern.notes[index][position.row],
                ) {
                    let target = targets.get(id).ok_or_else(|| {
                        format!("Apply the instrument assigned to {} first", track.name)
                    })?;
                    events.push(json!({"op":"note", "at":at, "target":target, "to":format!("track_{index}"), "tag":"tracker-pattern",
                        "dur":format!("{}sec", 60.0 / self.song.bpm / f64::from(self.song.rows_per_beat) * 0.8), "args":{"freq":pitch(note)?}}));
                }
            }
            self.next_row += 1;
        }
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
                    instrument: (i == 0).then(|| "tone".into()),
                    ..Track::default()
                })
                .collect(),
            patterns: [("a", 2, "C-4"), ("b", 3, "D#4")]
                .into_iter()
                .map(|(id, rows, note)| Pattern {
                    id: id.into(),
                    name: id.into(),
                    rows,
                    notes: (0..4).map(|_| vec![Some(note.into()); rows]).collect(),
                    fx: (0..4).map(|_| vec![FxLane::empty(rows)]).collect(),
                })
                .collect(),
            sequence: vec!["a".into(), "b".into(), "a".into()],
        }
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
