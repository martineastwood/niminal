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
control fn tempo_lfo(beats: num = 1, shape: (sine | tri | saw | square) = sine, phase: num = 0) {
  at = fract(clock.beat / beats + phase)
  if shape == sine { sin(at * 2 * pi) }
  else if shape == tri { abs(fract(at + 0.75) - 0.5) * 4 - 1 }
  else if shape == saw { fract(at + 0.5) * 2 - 1 }
  else { (at < 0.5) * 2 - 1 }
}
control fn random_steps(rate: hz, seed: num = 0) {
  noise(seed: seed).sample_hold(impulse(rate))
}
control fn random_glide(rate: hz, glide: sec = 200ms, seed: num = 0) {
  noise(seed: seed).sample_hold(impulse(rate)).lag(glide)
}
fn drum(
  freq: hz = 100hz,
  wave: (sine | tri | square | saw) = sine,
  attack: 0.1ms..50ms = 0.5ms,
  decay: 5ms..10sec = 300ms,
  mod_mode: (drop | wobble | random) = drop,
  mod_amount: -48..48 = 0,
  mod_rate: 1hz..4khz = 100hz,
  noise_filter: (low | band | high) = band,
  noise_freq: 20hz..20khz = 4khz,
  noise_q: 0..1 = 0.3,
  noise_shape: (soft | linear | burst) = soft,
  noise_attack: 0.1ms..200ms = 1ms,
  noise_decay: 5ms..10sec = 150ms,
  mix: 0..1 = 0,
  drive: 0..1 = 0,
  eq_freq: 20hz..20khz = 1khz,
  eq_gain: -24db..24db = 0db,
  amp: db = -6db,
  vel: 0..1 = 1,
  osc_vel: 0..1 = 0.5,
  noise_vel: 0..1 = 0.5,
  mod_vel: 0..1 = 0
) {
  swing = mod_amount * (1 - mod_vel * (1 - vel))
  bend = if mod_mode == drop {
    swing * perc(attack: 0.05ms, decay: 1sec / mod_rate.num)
  } else if mod_mode == wobble {
    osc(sine, mod_rate) * swing
  } else {
    noise().sample_hold(impulse(mod_rate)) * swing
  }
  cur_freq = freq * (bend * 0.0577623).exp
  tone = if wave == sine { osc(sine, cur_freq) } else if wave == tri { osc(tri, cur_freq) } else if wave == square { osc(square, cur_freq) } else { osc(saw, cur_freq) }
  body = tone * perc(attack: attack, decay: decay) * (1 - osc_vel * (1 - vel))
  hiss = noise()
  hiss = if noise_filter == low { hiss.lpf(cutoff: noise_freq, res: noise_q) } else if noise_filter == band { hiss.bpf(cutoff: noise_freq, res: noise_q) } else { hiss.hpf(cutoff: noise_freq, res: noise_q) }
  shape = if noise_shape == linear { env[0 noise_attack 1 noise_decay 0] } else { perc(attack: noise_attack, decay: noise_decay) }
  chop = if noise_shape == burst { osc(saw, mod_rate) * 0.5 + 0.5 } else { 1 }
  grit = hiss * shape * chop * (1 - noise_q * 0.5) * (1 - noise_vel * (1 - vel))
  voice = body * (1 - mix) + grit * mix
  voice = (voice * (1 + drive * 10)).tanh * (1 / (1 + drive * 2))
  voice.peak_eq(cutoff: eq_freq, gain: eq_gain, q: 1).tanh.gain(amp)
}
fn crossfade(x, wet, amount: 0..1) { x + (wet - x) * amount }
fn apply(x, make: fn, when: 0..1, blend: sec = 3ms) {
  state mix = 0
  step = 1 / (blend.num * sample_rate.num).max(1)
  mix = mix + (when - mix).max(-step).min(step)
  x + (make(x) - x) * mix
}
";

/// What each prelude function is for, for completion and the opcode reference.
pub const DOCS: [(&str, &str); 8] = [
    (
        "lfo",
        "A low-frequency oscillator from -1 to 1, run once per block: `lfo(2hz)`, or `lfo(0.3hz, shape: tri)` (sine, tri, saw or square). Move it to 0..1 with `.unipolar`, scale it with `.range(300hz..3khz)` and smooth it where a step would be heard: `lfo(2hz).smooth`.",
    ),
    (
        "tempo_lfo",
        "A low-frequency oscillator from -1 to 1 with a cycle every `beats` beats, locked to the beat count: `tempo_lfo(1/4)` is four a beat, `tempo_lfo(8)` one in two bars. A tempo change speeds it up or slows it down without it losing its place, and each cycle starts on a beat. `phase` (0 to 1) shifts it along a cycle and `shape` is sine, tri, saw or square. Runs once per block, like `lfo`: `tempo_lfo(1/2).unipolar.range(300hz..3khz).smooth(5ms)`.",
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
    (
        "drum",
        "A drum voice with the controls of a drum machine: an oscillator with a pitch modulation, a noise source with its own filter and envelope, a mix between them, then distortion and an EQ band. The drum presets in `std/drums` are settings of it. `mod_mode` is how the pitch moves (`drop` falls from `mod_amount` semitones above at `mod_rate`; `wobble` is a sine; `random` steps at random); `noise_shape` is how the noise swells and fades (`burst` chops it at `mod_rate`). `vel` is how hard the note is played (0..1); `osc_vel`, `noise_vel` and `mod_vel` say how much it matters to the oscillator, the noise and the pitch modulation.",
    ),
    (
        "crossfade",
        "Blends a sound with a processed copy of itself: 0 is only `x`, 1 is only `wet`. The dry/wet knob of an effect: `out = in.crossfade(in.lpf(cutoff: 800hz), mix)`.",
    ),
    (
        "apply",
        "Runs a function on the sound as much as `when` says: 0 is not at all, 1 is all of it, and a value between mixes the two. `when` moves smoothly over `blend` (3ms), so switching it never clicks. `x.apply(fn(s) { s.bitcrush(bits: 5) }, when: gate)`.",
    ),
];

/// The text of the prelude function `name` as it is written here, for copying into a project.
pub fn source_of(name: &str) -> Option<String> {
    let start = SOURCE.find(&format!("fn {name}("))?;
    let mut depth = 0;
    let mut end = None;
    for (i, c) in SOURCE[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let doc = DOCS.iter().find(|(n, _)| *n == name).map(|(_, d)| *d)?;
    Some(format!("/// {doc}\n{}\n", &SOURCE[start..end?]))
}

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

/// Whether `source` calls a prelude function that reads the clock, which needs the tempo controls declared.
pub fn reads_clock(source: &str) -> bool {
    source.contains("tempo_lfo")
}

/// The prelude functions `source` calls and `items` doesn't already define.
pub fn needed(source: &str, items: &[Item]) -> Vec<Item> {
    if !DOCS.iter().any(|(name, _)| source.contains(name)) {
        return Vec::new();
    }
    // spans and the names made from them (`<fn@40>`) must not collide with the file's own, so the
    // prelude is read as if it came after all of the file's text
    let shifted = format!("{}{}", " ".repeat(source.len() + 1), crate::dsp::lower_clock_reads(SOURCE));
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
