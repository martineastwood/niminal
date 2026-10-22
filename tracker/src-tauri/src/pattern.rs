use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Track {
    pub name: String,
    pub instrument: Option<String>,
    pub effect: Option<String>,
    pub muted: bool,
    pub volume_db: f64,
    pub notes: Vec<Option<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Pattern {
    pub bpm: f64,
    pub rows_per_beat: u32,
    pub tracks: Vec<Track>,
}

impl Pattern {
    pub fn validate(&self) -> Result<(), String> {
        if !self.bpm.is_finite()
            || !(20.0..=300.0).contains(&self.bpm)
            || !(1..=16).contains(&self.rows_per_beat)
            || self.tracks.len() != 4
            || self.tracks.iter().any(|track| {
                !track.volume_db.is_finite()
                    || !(-60.0..=6.0).contains(&track.volume_db)
                    || track.notes.len() != 32
            })
        {
            return Err("Expected 4 tracks of 32 rows, 20–300 BPM, and 1–16 rows per beat".into());
        }
        for track in &self.tracks {
            for note in track.notes.iter().flatten() {
                pitch(note)?;
            }
        }
        Ok(())
    }

    pub fn row_samples(&self, sample_rate: f64) -> f64 {
        sample_rate * 60.0 / self.bpm / f64::from(self.rows_per_beat)
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

pub struct Playback {
    pub origin: u64,
    pub next_row: u64,
    pub pattern: Pattern,
}

impl Playback {
    pub fn row_at(&self, index: u64, sample_rate: f64) -> u64 {
        self.origin + (index as f64 * self.pattern.row_samples(sample_rate)).round() as u64
    }

    pub fn position(&self, sample: u64, sample_rate: f64) -> Option<usize> {
        (sample >= self.origin).then(|| {
            ((sample - self.origin) as f64 / self.pattern.row_samples(sample_rate)).floor() as usize
                % 32
        })
    }

    // Derive every timestamp from the origin to avoid accumulated rounding drift.
    // If a network stall loses the scheduling window, skip missed rows rather
    // than submitting a burst of overdue notes.
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
            let row = self.next_row as usize % 32;
            self.next_row += 1;
            if at < sample {
                skipped += 1;
                continue;
            }
            for (index, track) in self.pattern.tracks.iter().enumerate() {
                if let (Some(id), Some(note)) = (&track.instrument, &track.notes[row]) {
                    let target = targets.get(id).ok_or_else(|| {
                        format!("Apply the instrument assigned to {} first", track.name)
                    })?;
                    events.push(json!({
                        "op":"note", "at":at, "target":target, "to":format!("track_{index}"), "tag":"tracker-pattern",
                        "dur":format!("{}sec", 60.0 / self.pattern.bpm / f64::from(self.pattern.rows_per_beat) * 0.8),
                        "args":{"freq":pitch(note)?}
                    }));
                }
            }
        }
        Ok((events, skipped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern() -> Pattern {
        Pattern {
            bpm: 137.0,
            rows_per_beat: 4,
            tracks: (0..4)
                .map(|i| Track {
                    name: format!("Track {i}"),
                    effect: None,
                    muted: false,
                    volume_db: 0.0,
                    instrument: Some("tone".into()),
                    notes: (0..32).map(|_| Some("C#4".into())).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn timestamps_stay_on_the_musical_grid_across_loops() {
        let mut play = Playback {
            origin: 4800,
            next_row: 31,
            pattern: pattern(),
        };
        let targets = BTreeMap::from([("tone".into(), "voice".into())]);
        let horizon = play.row_at(33, 48000.0);
        let (events, skipped) = play.events_until(4800, horizon, 48000.0, &targets).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(events.len(), 8);
        assert_eq!(events[0]["at"], play.row_at(31, 48000.0));
        assert_eq!(events[4]["at"], play.row_at(32, 48000.0));
        assert_eq!(events[4]["args"]["freq"], "c#4");
        assert_eq!(
            play.row_at(100_000, 48000.0),
            4800 + (100_000.0_f64 * 48000.0 * 60.0 / 137.0 / 4.0).round() as u64
        );
        assert_eq!(play.position(4799, 48000.0), None);
        assert_eq!(
            play.position(play.row_at(32, 48000.0) + 1, 48000.0),
            Some(0)
        );
    }

    #[test]
    fn a_stall_skips_overdue_rows_without_replaying_them() {
        let mut play = Playback {
            origin: 0,
            next_row: 0,
            pattern: pattern(),
        };
        let targets = BTreeMap::from([("tone".into(), "voice".into())]);
        let (events, skipped) = play
            .events_until(
                play.row_at(5, 48000.0),
                play.row_at(6, 48000.0),
                48000.0,
                &targets,
            )
            .unwrap();
        assert_eq!(skipped, 5);
        assert_eq!(events.len(), 4);
        assert_eq!(events[0]["at"], play.row_at(5, 48000.0));
    }
}
