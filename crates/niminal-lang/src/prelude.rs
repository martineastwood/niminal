//! Functions every file can call without defining them. Each is only added when the file uses its
//! name and defines nothing of that name itself, so a file's own `fn lfo` wins.

use crate::ast::Item;
use crate::parser;

const SOURCE: &str = "
control fn lfo(rate: hz, shape: (sine | tri | saw | square) = sine) {
  if shape == sine { osc(sine, rate) }
  else if shape == tri { osc(tri, rate) }
  else if shape == saw { osc(saw, rate) }
  else { osc(square, rate) }
}
fn unipolar(x) { x * 0.5 + 0.5 }
control fn random_steps(rate: hz, seed: num = 0) {
  noise(seed: seed).sample_hold(impulse(rate))
}
control fn random_glide(rate: hz, glide: sec = 200ms, seed: num = 0) {
  noise(seed: seed).sample_hold(impulse(rate)).lag(glide)
}
";

/// What each prelude function is for, for completion and the opcode reference.
pub const DOCS: [(&str, &str); 4] = [
    (
        "lfo",
        "A low-frequency oscillator from -1 to 1, run once per block: `lfo(2hz)`, or `lfo(0.3hz, shape: tri)` (sine, tri, saw or square). Move it to 0..1 with `.unipolar`, scale it with `.range(300hz..3khz)` and smooth it where a step would be heard: `lfo(2hz).smooth`.",
    ),
    (
        "unipolar",
        "Moves a signal from -1..1 to 0..1, ready for `.range`: `lfo(0.3hz).unipolar.range(300hz..3khz)`.",
    ),
    (
        "random_steps",
        "Random values from -1 to 1, a new one at `rate`, held between: `random_steps(4hz)`. `seed:` makes a different sequence. Runs once per block.",
    ),
    (
        "random_glide",
        "Random values from -1 to 1 that slide to each new one over `glide` (200ms unless given): `random_glide(3hz)`. Runs once per block.",
    ),
];

fn defined_name(item: &Item) -> Option<&str> {
    Some(match item {
        Item::Control { name, .. } | Item::Const { name, .. } => &name.name,
        Item::Instr(d) => &d.name.name,
        Item::MidiOut(d) => &d.name.name,
        Item::Fn(d) => &d.name.name,
        Item::Bus(d) => &d.name.name,
        Item::Buffer(d) => &d.name.name,
        Item::Node(d) | Item::Fx(d) => &d.name.name,
        Item::Sample(d) => &d.name.name,
        _ => return None,
    })
}

/// The prelude functions `source` calls and `items` doesn't already define.
pub fn needed(source: &str, items: &[Item]) -> Vec<Item> {
    if !DOCS.iter().any(|(name, _)| source.contains(name)) {
        return Vec::new();
    }
    // spans and the names made from them (`<fn@40>`) must not collide with the file's own, so the
    // prelude is read as if it came after all of the file's text
    let shifted = format!("{}{SOURCE}", " ".repeat(source.len() + 1));
    let parsed = parser::parse(&shifted).expect("the prelude parses");
    parsed
        .into_iter()
        .filter(|item| {
            defined_name(item).is_some_and(|name| {
                source.contains(name) && !items.iter().any(|i| defined_name(i) == Some(name))
            })
        })
        .collect()
}
