//! A drum grid made to order: pick a theme, how busy it is, how much it wanders, and a seed. The same
//! choices always make the same grid, which is written out as an ordinary `grid { ... }` to edit.
//!
//! Each theme is a table of how much every step of a bar wants a hit, for each drum: 9 is the anchor of
//! the groove, 1 a ghost note, 0 never. A step hits when its weight clears the drum's cutoff, so thinning
//! a groove takes out the ghosts first and the anchors last, and a busy drum thins slower than a sparse one.

/// Steps in a bar (sixteenth notes).
pub const STEPS: usize = 16;

/// A drum's weights: a digit for each step.
type Weights = &'static str;

struct Theme {
    name: &'static str,
    /// Drum name and the weights of its steps, spaces ignored.
    rows: &'static [(&'static str, Weights)],
}

const THEMES: &[Theme] = &[
    Theme {
        name: "techno",
        rows: &[
            ("kick", "9000 9000 9000 9000"),
            ("clap", "0000 9000 0000 9000"),
            ("hat", "3535 3535 3535 3535"),
            ("open", "0040 0040 0040 0040"),
            ("perc", "0010 0200 0100 0200"),
        ],
    },
    Theme {
        name: "house",
        rows: &[
            ("kick", "9000 9000 9000 9000"),
            ("clap", "0000 9000 0000 9000"),
            ("hat", "5353 5353 5353 5353"),
            ("open", "0090 0090 0090 0090"),
            ("perc", "0000 0200 0001 0200"),
        ],
    },
    Theme {
        name: "two_step",
        rows: &[
            ("kick", "9000 0050 0090 0000"),
            ("snare", "0000 9000 0000 9003"),
            ("hat", "5353 5353 5353 5353"),
            ("open", "0000 0000 0000 0040"),
            ("perc", "0200 0010 0200 0001"),
        ],
    },
    Theme {
        name: "halftime",
        rows: &[
            ("kick", "9000 0000 0050 0006"),
            ("snare", "0000 0000 9000 0000"),
            ("hat", "5050 5050 5050 5050"),
            ("open", "0000 0000 0000 0030"),
            ("perc", "0000 0200 0000 0100"),
        ],
    },
    Theme {
        name: "breaks",
        rows: &[
            ("kick", "9000 0050 0900 0030"),
            ("snare", "0000 9002 0200 9005"),
            ("hat", "5353 5353 5353 5353"),
            ("open", "0000 0040 0000 0040"),
            ("perc", "0100 0020 0100 0020"),
        ],
    },
    Theme {
        name: "boom_bap",
        rows: &[
            ("kick", "9000 0050 9500 0000"),
            ("snare", "0000 9000 0003 9000"),
            ("hat", "6060 6060 6060 6060"),
            ("open", "0000 0000 0000 0030"),
            ("perc", "0000 0000 0200 0000"),
        ],
    },
    Theme {
        name: "trap",
        rows: &[
            ("kick", "9000 0050 0000 5004"),
            ("clap", "0000 0000 9000 0000"),
            ("hat", "7777 7777 7777 7777"),
            ("open", "0000 0000 0000 0050"),
            ("perc", "0010 0000 0100 0000"),
        ],
    },
    Theme {
        name: "dnb",
        rows: &[
            ("kick", "9000 0000 0090 0000"),
            ("snare", "0000 9000 0000 9000"),
            ("hat", "5050 5050 5050 5050"),
            ("open", "0000 0000 0000 0040"),
            ("perc", "0200 0000 0020 0000"),
        ],
    },
];

/// The themes, by name.
pub fn themes() -> Vec<&'static str> {
    THEMES.iter().map(|t| t.name).collect()
}

/// What to make.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub theme: &'a str,
    /// How busy it is, 0 (only the anchors, or nothing) to 1 (every step the theme wants).
    pub density: f64,
    /// How far it wanders from the theme, 0 (exactly the theme) to 1 (a different groove each seed).
    pub chaos: f64,
    pub seed: u64,
    /// Drums to make busier (above 1) or sparser (below 1) than `density` says, by name.
    pub per_drum: &'a [(String, f64)],
}

impl Default for Request<'_> {
    fn default() -> Self {
        Request {
            theme: "techno",
            density: 0.7,
            chaos: 0.0,
            seed: 1,
            per_drum: &[],
        }
    }
}

/// A drum's steps: `x` a hit, `o` a ghost, `.` nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub name: &'static str,
    pub steps: [char; STEPS],
}

/// A number from the seed, the drum and the step alone, between -1 and 1.
fn jitter(seed: u64, drum: usize, step: usize) -> f64 {
    let mut z = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((drum as u64) << 32 | step as u64)
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

/// The grid for a request, or what is wrong with it.
pub fn generate(request: &Request) -> Result<Vec<Row>, String> {
    let theme = THEMES
        .iter()
        .find(|t| t.name == request.theme)
        .ok_or_else(|| {
            format!(
                "no theme `{}`; the themes are {}",
                request.theme,
                themes().join(", ")
            )
        })?;
    for (name, _) in request.per_drum {
        if !theme.rows.iter().any(|(drum, _)| drum == name) {
            let drums: Vec<&str> = theme.rows.iter().map(|(d, _)| *d).collect();
            return Err(format!(
                "the theme `{}` has no drum `{name}`; its drums are {}",
                theme.name,
                drums.join(", ")
            ));
        }
    }
    let (density, chaos) = (request.density.clamp(0.0, 1.0), request.chaos.clamp(0.0, 1.0));
    let mut rows = Vec::new();
    for (d, (name, weights)) in theme.rows.iter().enumerate() {
        let scale = request
            .per_drum
            .iter()
            .find(|(drum, _)| drum == name)
            .map_or(1.0, |(_, s)| *s);
        let wanted = (density * scale).clamp(0.0, 1.0);
        // a step hits when its weight, nudged by chaos, reaches this
        let cutoff = 1.0 - wanted;
        let mut steps = ['.'; STEPS];
        let digits = weights.chars().filter(|c| c.is_ascii_digit());
        for (i, digit) in digits.take(STEPS).enumerate() {
            let weight = f64::from(digit.to_digit(10).unwrap_or(0)) / 9.0;
            // chaos moves steps up or down, and gives an empty step a chance
            let score = weight + chaos * 0.6 * jitter(request.seed, d, i);
            if wanted > 0.0 && score >= cutoff.max(0.05) && (weight > 0.0 || chaos > 0.0) {
                steps[i] = if weight >= 0.5 || (weight == 0.0 && score >= 0.5) {
                    'x'
                } else {
                    'o'
                };
            }
        }
        rows.push(Row { name, steps });
    }
    Ok(rows)
}

/// The rows as a `grid { ... }`, in groups of four steps.
pub fn grid_text(rows: &[Row]) -> String {
    let width = rows.iter().map(|r| r.name.len()).max().unwrap_or(0) + 1;
    let mut text = String::from("grid {\n");
    for row in rows {
        let steps: Vec<String> = row.steps.chunks(4).map(|c| c.iter().collect()).collect();
        text.push_str(&format!(
            "  {:<width$} {}\n",
            format!("{}:", row.name),
            steps.join(" ")
        ));
    }
    text.push('}');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(theme: &str, density: f64, chaos: f64, seed: u64) -> Vec<Row> {
        generate(&Request {
            theme,
            density,
            chaos,
            seed,
            per_drum: &[],
        })
        .unwrap()
    }

    fn hits(rows: &[Row], drum: &str) -> usize {
        rows.iter()
            .find(|r| r.name == drum)
            .unwrap()
            .steps
            .iter()
            .filter(|c| **c != '.')
            .count()
    }

    #[test]
    fn four_on_the_floor_techno_is_the_theme_at_full_density() {
        let rows = make("techno", 1.0, 0.0, 1);
        let kick: String = rows[0].steps.iter().collect();
        assert_eq!(kick, "x...x...x...x...");
    }

    #[test]
    fn thinning_takes_the_ghosts_first_and_the_anchors_last() {
        let full = make("boom_bap", 1.0, 0.0, 1);
        let thin = make("boom_bap", 0.4, 0.0, 1);
        let bare = make("boom_bap", 0.1, 0.0, 1);
        assert!(hits(&full, "snare") > hits(&thin, "snare"));
        assert_eq!(hits(&thin, "snare"), 2, "the backbeat stays");
        assert_eq!(hits(&bare, "snare"), 2, "the backbeat stays");
        assert_eq!(hits(&make("boom_bap", 0.0, 0.0, 1), "snare"), 0);
        // fewer steps never turn up when it thins
        for drum in ["kick", "snare", "hat"] {
            assert!(hits(&full, drum) >= hits(&thin, drum));
        }
    }

    #[test]
    fn the_same_choices_make_the_same_grid_and_another_seed_a_different_one() {
        assert_eq!(make("house", 0.8, 0.5, 7), make("house", 0.8, 0.5, 7));
        assert_ne!(make("house", 0.8, 0.5, 7), make("house", 0.8, 0.5, 8));
        // without chaos the seed makes no difference
        assert_eq!(make("house", 0.8, 0.0, 7), make("house", 0.8, 0.0, 8));
    }

    #[test]
    fn a_drum_can_be_made_sparser_than_the_rest() {
        let busy = generate(&Request {
            theme: "house",
            density: 0.9,
            ..Request::default()
        })
        .unwrap();
        let thin = generate(&Request {
            theme: "house",
            density: 0.9,
            per_drum: &[("hat".to_string(), 0.4)],
            ..Request::default()
        })
        .unwrap();
        assert!(hits(&thin, "hat") < hits(&busy, "hat"));
        assert_eq!(hits(&thin, "kick"), hits(&busy, "kick"));
    }

    #[test]
    fn every_theme_makes_a_kick_and_a_backbeat_and_text_that_is_a_grid() {
        for name in themes() {
            let rows = make(name, 0.7, 0.0, 1);
            assert!(hits(&rows, "kick") > 0, "{name}");
            assert!(grid_text(&rows).starts_with("grid {\n"), "{name}");
            assert!(rows.iter().all(|r| r.steps.len() == STEPS));
        }
    }

    #[test]
    fn mistakes_name_what_there_is() {
        let e = generate(&Request {
            theme: "polka",
            ..Request::default()
        })
        .unwrap_err();
        assert!(e.contains("techno") && e.contains("trap"), "{e}");
        let e = generate(&Request {
            per_drum: &[("tuba".to_string(), 1.0)],
            ..Request::default()
        })
        .unwrap_err();
        assert!(e.contains("kick"), "{e}");
    }
}
