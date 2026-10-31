//! The gallery: many versions of one sample, each a short piece of ordinary niminal code.
//!
//! A recipe is a template in `gallery/`. Its comment header names its family and gives each setting a
//! range (`// density: 8..120 log`, `int` for whole numbers), and the body uses `{density}`, `{seed}` and
//! `{sample}`. A version is the template with each setting picked at random inside its range.

use std::fmt::Write;

use serde_json::{Value, json};

const RECIPES: &[(&str, &str)] = &[
    ("cloud", include_str!("../gallery/cloud.nml")),
    ("freeze", include_str!("../gallery/freeze.nml")),
    ("fragments", include_str!("../gallery/fragments.nml")),
    ("debris", include_str!("../gallery/debris.nml")),
    ("wave_voice", include_str!("../gallery/wave_voice.nml")),
    ("pitch_shift", include_str!("../gallery/pitch_shift.nml")),
    (
        "spectral_hold",
        include_str!("../gallery/spectral_hold.nml"),
    ),
    ("echoes", include_str!("../gallery/echoes.nml")),
    ("sweep", include_str!("../gallery/sweep.nml")),
    ("stutter", include_str!("../gallery/stutter.nml")),
    ("rewind", include_str!("../gallery/rewind.nml")),
    ("tape_stop", include_str!("../gallery/tape_stop.nml")),
    ("shuffle", include_str!("../gallery/shuffle.nml")),
    ("crush", include_str!("../gallery/crush.nml")),
    ("glitch_chain", include_str!("../gallery/glitch_chain.nml")),
];

/// How many versions of each recipe to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Amount {
    Few,
    Enough,
    Lots,
}

impl Amount {
    fn versions(self) -> usize {
        match self {
            Amount::Few => 1,
            Amount::Enough => 3,
            Amount::Lots => 6,
        }
    }
}

/// One setting of a recipe and the range it is picked from.
struct Setting {
    name: String,
    low: f64,
    high: f64,
    log: bool,
    whole: bool,
}

struct Recipe<'a> {
    name: &'a str,
    family: &'a str,
    settings: Vec<Setting>,
    template: &'a str,
}

/// One version: a file's worth of code, and what it was made from.
pub struct Version {
    /// `cloud_2`, the name of its files.
    pub name: String,
    pub recipe: String,
    pub family: String,
    pub values: Vec<(String, f64)>,
    pub source: String,
}

impl Version {
    pub fn json(&self) -> Value {
        json!({
            "name": self.name,
            "recipe": self.recipe,
            "family": self.family,
            "values": self.values.iter().map(|(n, v)| (n.clone(), json!(v))).collect::<serde_json::Map<_, _>>(),
        })
    }
}

/// The families a recipe can belong to.
pub fn families() -> Vec<String> {
    let mut found: Vec<String> = recipes().iter().map(|r| r.family.to_string()).collect();
    found.sort();
    found.dedup();
    found
}

fn recipes() -> Vec<Recipe<'static>> {
    RECIPES
        .iter()
        .map(|(name, text)| parse(name, text))
        .collect()
}

fn parse(name: &'static str, text: &'static str) -> Recipe<'static> {
    let mut family = "";
    let mut settings = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("// ") else {
            continue;
        };
        let Some((name, spec)) = rest.split_once(": ") else {
            continue;
        };
        if name == "family" {
            family = spec.trim();
        } else if let Some((low, tail)) = spec.split_once("..") {
            let mut words = tail.split_whitespace();
            let (Ok(low), Some(Ok(high))) = (low.parse(), words.next().map(str::parse)) else {
                continue;
            };
            let flags: Vec<&str> = words.collect();
            settings.push(Setting {
                name: name.to_string(),
                low,
                high,
                log: flags.contains(&"log"),
                whole: flags.contains(&"int"),
            });
        }
    }
    Recipe {
        name,
        family,
        settings,
        template: text,
    }
}

/// A small repeatable random source.
struct Random(u64);

impl Random {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A number as short as it can be written, to three decimals.
fn number(value: f64) -> String {
    let text = format!("{value:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// The versions to make of `sample`: `amount` of each recipe in `families` (all when empty), the same
/// for the same `seed`.
pub fn generate(sample: &str, families: &[String], amount: Amount, seed: u64) -> Vec<Version> {
    let mut versions = Vec::new();
    for (index, recipe) in recipes().into_iter().enumerate() {
        if !families.is_empty() && !families.iter().any(|f| f == recipe.family) {
            continue;
        }
        for k in 0..amount.versions() {
            let version_seed = seed
                .wrapping_mul(1000)
                .wrapping_add((index * 100 + k) as u64);
            let mut random = Random(version_seed);
            let mut values = Vec::new();
            let mut source = format!(
                "// {} ({}), version {}: made by `niminal-live gallery` from {sample}.\n",
                recipe.name,
                recipe.family,
                k + 1
            );
            let mut body = recipe.template.to_string();
            for setting in &recipe.settings {
                let t = random.next();
                let mut value = if setting.log {
                    setting.low * (setting.high / setting.low).powf(t)
                } else {
                    setting.low + (setting.high - setting.low) * t
                };
                if setting.whole {
                    value = value.round();
                }
                let text = number(value);
                values.push((setting.name.clone(), text.parse().unwrap_or(value)));
                body = body.replace(&format!("{{{}}}", setting.name), &text);
            }
            body = body
                .replace("{seed}", &(version_seed % 997).to_string())
                .replace("{sample}", sample);
            source.push_str(&body);
            let mut name = String::new();
            let _ = write!(name, "{}_{}", recipe.name, k + 1);
            versions.push(Version {
                name,
                recipe: recipe.name.to_string(),
                family: recipe.family.to_string(),
                values,
                source,
            });
        }
    }
    versions
}
