//! Musical tables and helpers: built-in signals, scales, chords and arpeggio orders.

use super::*;

/// The built-in signals: `sine`, `saw`, `tri`, `square`, `rand` and `perlin`, each from 0 to 1.
pub(super) fn signal(name: &str) -> Option<Pattern<f64>> {
    Some(match name {
        "sine" => Pattern::sine(),
        "saw" => Pattern::saw(),
        "tri" => Pattern::tri(),
        "square" => Pattern::square(),
        "rand" => Pattern::rand(),
        "perlin" => Pattern::perlin(),
        _ => return None,
    })
}

/// Scales as semitones above the root, one octave.
pub(super) const SCALES: [(&str, &[i32]); 14] = [
    ("major", &[0, 2, 4, 5, 7, 9, 11]),
    ("minor", &[0, 2, 3, 5, 7, 8, 10]),
    ("harmonic_minor", &[0, 2, 3, 5, 7, 8, 11]),
    ("melodic_minor", &[0, 2, 3, 5, 7, 9, 11]),
    ("dorian", &[0, 2, 3, 5, 7, 9, 10]),
    ("phrygian", &[0, 1, 3, 5, 7, 8, 10]),
    ("lydian", &[0, 2, 4, 6, 7, 9, 11]),
    ("mixolydian", &[0, 2, 4, 5, 7, 9, 10]),
    ("locrian", &[0, 1, 3, 5, 6, 8, 10]),
    ("major_pentatonic", &[0, 2, 4, 7, 9]),
    ("minor_pentatonic", &[0, 3, 5, 7, 10]),
    ("blues", &[0, 3, 5, 6, 7, 10]),
    ("whole_tone", &[0, 2, 4, 6, 8, 10]),
    ("chromatic", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
];

/// The note of the scale (rooted at `root`, in any octave) nearest to `note`; a note halfway between two goes down.
pub(super) fn snap_to_scale(note: i32, root: i32, scale: &[i32]) -> i32 {
    let octave = (note - root).div_euclid(12);
    (octave - 1..=octave + 1)
        .flat_map(|o| scale.iter().map(move |degree| root + 12 * o + degree))
        .min_by_key(|candidate| ((candidate - note).abs(), *candidate))
        .unwrap_or(note)
}

/// Chords as semitones above the root.
pub(super) const CHORDS: [(&str, &[i32]); 16] = [
    ("major", &[0, 4, 7]),
    ("minor", &[0, 3, 7]),
    ("dim", &[0, 3, 6]),
    ("aug", &[0, 4, 8]),
    ("sus2", &[0, 2, 7]),
    ("sus4", &[0, 5, 7]),
    ("power", &[0, 7]),
    ("major6", &[0, 4, 7, 9]),
    ("minor6", &[0, 3, 7, 9]),
    ("major7", &[0, 4, 7, 11]),
    ("minor7", &[0, 3, 7, 10]),
    ("dom7", &[0, 4, 7, 10]),
    ("dim7", &[0, 3, 6, 9]),
    ("half_dim7", &[0, 3, 6, 10]),
    ("major9", &[0, 4, 7, 11, 14]),
    ("minor9", &[0, 3, 7, 10, 14]),
];

/// Split a chord name into a root (octave 4 by default) and interval offsets.
pub(super) fn chord_notes(atom: &str) -> Option<Vec<Value>> {
    let bytes = atom.as_bytes();
    if !matches!(bytes.first()?, b'A'..=b'G' | b'a'..=b'g') {
        return None;
    }
    let mut end = 1;
    if matches!(bytes.get(end), Some(b'#' | b'b')) {
        end += 1;
    }
    let root_end = end;
    if bytes.get(end) == Some(&b'-') {
        end += 1;
    }
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    let quality = match &atom[end..] {
        "maj" => "major",
        "min" | "m" => "minor",
        "maj7" => "major7",
        "min7" | "m7" => "minor7",
        "maj6" => "major6",
        "min6" | "m6" => "minor6",
        "maj9" => "major9",
        "min9" | "m9" => "minor9",
        name => name,
    };
    let intervals = CHORDS.iter().find(|(name, _)| *name == quality)?.1;
    let root = if end == root_end {
        format!("{}4", &atom[..root_end])
    } else {
        atom[..end].to_string()
    };
    let Value::Note(root) = root.to_ascii_lowercase().parse::<Value>().ok()? else {
        return None;
    };
    Some(
        intervals
            .iter()
            .map(|offset| Value::Note(root + offset))
            .collect(),
    )
}

/// How arpeggios can run, lowest note first.
/// Whose timing `add`, `sub`, `mul` and `div` keep.
pub(super) const TIMINGS: [(&str, &Structure); 3] = [
    ("first", &Structure::Left),
    ("second", &Structure::Right),
    ("both", &Structure::Both),
];

pub(super) const ARP_ORDERS: [(&str, &str); 6] = [
    ("up", "up"),
    ("down", "down"),
    ("updown", "updown"),
    ("downup", "downup"),
    ("converge", "converge"),
    ("diverge", "diverge"),
];

pub(super) fn arpeggio_order(order: &str, mut notes: Vec<Step>) -> Vec<Step> {
    notes.sort_by(|a, b| pitch(&a.value).total_cmp(&pitch(&b.value)));
    let init = |v: &[Step]| v[..v.len().saturating_sub(1).max(1)].to_vec();
    let reversed = |v: &[Step]| v.iter().rev().cloned().collect::<Vec<_>>();
    if notes.len() < 2 {
        return notes;
    }
    // outside in: lowest, highest, next lowest...
    let converge = |v: &[Step]| {
        let (mut lo, mut hi, mut out) = (0, v.len(), Vec::new());
        while lo < hi {
            out.push(v[lo]);
            lo += 1;
            if lo < hi {
                hi -= 1;
                out.push(v[hi]);
            }
        }
        out
    };
    match order {
        "down" => reversed(&notes),
        "updown" => [init(&notes), init(&reversed(&notes))].concat(),
        "downup" => [init(&reversed(&notes)), init(&notes)].concat(),
        "converge" => converge(&notes),
        "diverge" => reversed(&converge(&notes)),
        _ => notes,
    }
}
