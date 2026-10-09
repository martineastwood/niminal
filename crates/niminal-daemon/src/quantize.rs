//! When changes land: the musical boundary a change waits for.

use niminal_lang::{Quantize, QuantizeRelation, QuantizeUnit, Statement, StatementKind};

/// The quantum each kind of change uses when it doesn't say.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuantizeDefaults {
    pub default: Quantize,
    /// Starting a clip: at the next cycle of the bar grid.
    pub play: Quantize,
    pub instrument: Quantize,
    pub track: Quantize,
    pub tempo: Quantize,
}

fn every(count: f64, unit: QuantizeUnit) -> Quantize {
    Quantize { relation: QuantizeRelation::OnGrid, count, unit }
}

impl Default for QuantizeDefaults {
    fn default() -> Self {
        QuantizeDefaults {
            default: every(1.0, QuantizeUnit::Bar),
            play: every(1.0, QuantizeUnit::Cycle),
            instrument: every(1.0, QuantizeUnit::Beat),
            track: every(1.0, QuantizeUnit::Bar),
            tempo: every(1.0, QuantizeUnit::Bar),
        }
    }
}

impl QuantizeDefaults {
    /// The quantum for a statement: its own `@`, else `fallback` (an override
    /// for the whole evaluation), else the default for its kind.
    pub fn for_statement(&self, s: &Statement, fallback: Option<Quantize>) -> Quantize {
        if let Some(q) = s.quantize.or(fallback) {
            return q;
        }
        match s.kind {
            StatementKind::Note => every(1.0, QuantizeUnit::Now),
            StatementKind::Command if s.plays.is_some() => self.play,
            StatementKind::Command => self.default,
            StatementKind::Definition => match s.key.as_deref().and_then(|k| k.split(':').next()) {
                Some("instr") => self.instrument,
                Some("track") => self.track,
                Some("tempo") => self.tempo,
                _ => self.default,
            },
        }
    }
}

/// The musical grid of a session.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub sample_rate: f64,
    pub bpm: f64,
    pub beats_per_bar: f64,
}

impl Grid {
    pub fn beat_seconds(&self) -> f64 {
        60.0 / self.bpm
    }

    pub fn bar_seconds(&self) -> f64 {
        self.beat_seconds() * self.beats_per_bar
    }

    /// The first sample at or after `now` where a change with this quantum may
    /// land. `now` is a sample count.
    pub fn landing(&self, q: Quantize, now: u64) -> u64 {
        let unit = match q.unit {
            QuantizeUnit::Now => return now,
            QuantizeUnit::Beat => self.beat_seconds(),
            QuantizeUnit::Bar | QuantizeUnit::Cycle => self.bar_seconds(),
        };
        let grid = (q.count * unit * self.sample_rate).round().max(1.0);
        let now_f = now as f64;
        let target = match q.relation {
            // the next line of the grid, at or after now
            QuantizeRelation::OnGrid => (now_f / grid).ceil() * grid,
            // the next line strictly after now
            QuantizeRelation::Next => ((now_f / grid).floor() + 1.0) * grid,
            QuantizeRelation::In => now_f + grid,
        };
        target as u64
    }

    /// `bar:beat` of a sample, counting from 1.
    pub fn position(&self, sample: u64) -> (u64, f64) {
        let beats = sample as f64 / self.sample_rate / self.beat_seconds();
        let bar = (beats / self.beats_per_bar).floor();
        (bar as u64 + 1, beats - bar * self.beats_per_bar + 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use niminal_lang::analyze;

    fn grid() -> Grid {
        // 120bpm in 4/4: a bar is 2 seconds, 96000 samples
        Grid { sample_rate: 48_000.0, bpm: 120.0, beats_per_bar: 4.0 }
    }

    fn q(text: &str) -> Quantize {
        analyze(&format!("hush @ {text}")).unwrap()[0].quantize.unwrap()
    }

    #[test]
    fn the_grid_has_the_right_lengths() {
        assert_eq!(grid().beat_seconds(), 0.5);
        assert_eq!(grid().bar_seconds(), 2.0);
    }

    #[test]
    fn next_lands_strictly_after_now_and_on_grid_at_or_after() {
        let g = grid();
        assert_eq!(g.landing(q("next bar"), 10_000), 96_000);
        assert_eq!(g.landing(q("next bar"), 96_000), 192_000, "already on the line: the following one");
        assert_eq!(g.landing(q("bar"), 96_000), 96_000, "plain `bar` may land right now");
        assert_eq!(g.landing(q("bar"), 96_001), 192_000);
        assert_eq!(g.landing(q("next 4 bars"), 10_000), 384_000);
        assert_eq!(g.landing(q("next 4 bars"), 400_000), 768_000);
        assert_eq!(g.landing(q("next beat"), 10_000), 24_000);
        assert_eq!(g.landing(q("4 bars"), 100_000), 384_000);
    }

    #[test]
    fn in_counts_from_now_and_now_is_now() {
        let g = grid();
        assert_eq!(g.landing(q("in 4 bars"), 10_000), 394_000);
        assert_eq!(g.landing(q("in 2 beats"), 5), 48_005);
        assert_eq!(g.landing(q("now"), 12_345), 12_345);
        assert_eq!(g.landing(q("cycle"), 1), 96_000);
    }

    #[test]
    fn positions_count_bars_and_beats_from_one() {
        let g = grid();
        assert_eq!(g.position(0), (1, 1.0));
        assert_eq!(g.position(24_000), (1, 2.0));
        assert_eq!(g.position(96_000), (2, 1.0));
        let (bar, beat) = g.position(96_000 * 3 + 36_000);
        assert_eq!((bar, beat), (4, 2.5));
        let waltz = Grid { beats_per_bar: 3.0, ..g };
        assert_eq!(waltz.position(72_000), (2, 1.0));
    }

    #[test]
    fn defaults_follow_the_kind_of_change() {
        let d = QuantizeDefaults::default();
        let first = |src: &str| analyze(src).unwrap().remove(0);
        let unit = |src: &str| d.for_statement(&first(src), None).unit;
        assert_eq!(unit("instr a(f: hz) { osc(sine, f) }"), QuantizeUnit::Beat);
        assert_eq!(unit("track t { out = it }"), QuantizeUnit::Bar);
        assert_eq!(unit("play t = [c4]"), QuantizeUnit::Cycle);
        assert_eq!(unit("mute t"), QuantizeUnit::Bar);
        assert_eq!(unit("a(f: 1hz) for 1beat"), QuantizeUnit::Now);
        assert_eq!(unit("riff = [c4]"), QuantizeUnit::Bar);
        // a statement's own `@` wins, then an override, then the kind's default
        let own = first("mute t @ next beat");
        let over = Some(q("now"));
        assert_eq!(d.for_statement(&own, over).unit, QuantizeUnit::Beat);
        assert_eq!(d.for_statement(&first("mute t"), over).unit, QuantizeUnit::Now);
    }
}
