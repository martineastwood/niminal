//! Bounded particle synthesis: four readers, trainlets, grain masks and clock synchronization.
mod mask;
#[cfg(test)]
mod tests;
mod trainlet;

use super::Interp;
use crate::{
    Table,
    opcode::{Opcode, Port, ProcessCtx},
    table::Edge,
};
use mask::Mask;
use std::sync::Arc;

const CAPACITY: usize = 256;
const MAX_GRAINS: usize = 4096;
const MASKS: usize = 6;
/// Fields appended to the audio outputs when monitoring is enabled.
pub const GRANULAR_MONITOR_FIELDS: [&str; 8] = [
    "sync",
    "phase",
    "gain_index",
    "start_index",
    "end_index",
    "fm_index",
    "channel_index",
    "mix_index",
];
const PORTS: &[Port] = &[
    Port::optional("density", 20.0),
    Port::optional("duration", 0.1),
    Port::optional("position", 0.0),
    Port::optional("rate", 1.0),
    Port::optional("spray", 0.0),
    Port::optional("pitch_scatter", 0.0),
    Port::optional("jitter", 0.0),
    Port::optional("trigger", 0.0),
    Port::optional("shape", 0.5),
    Port::optional("sweep", 0.0),
    Port::optional("pan", 0.0),
    Port::optional("spread", 0.0),
    Port::optional("fm", 0.0),
    Port::optional("fm_depth", 0.0),
    Port::optional("mix", 0.0),
    Port::optional("gain", 1.0),
    Port::optional("probability", 1.0),
    Port::optional("position2", f32::INFINITY),
    Port::optional("pitch2", 0.0),
    Port::optional("sync", 0.0),
    Port::optional("distribution", 0.0),
    Port::optional("sustain", 0.0),
    Port::optional("window_amount", 1.0),
    Port::optional("sweep_shape", f32::INFINITY),
    Port::optional("pitch_start", 0.0),
    Port::optional("pitch_end", f32::INFINITY),
    Port::optional("pitch", 0.0),
    Port::optional("position3", f32::INFINITY),
    Port::optional("pitch3", 0.0),
    Port::optional("position4", f32::INFINITY),
    Port::optional("pitch4", 0.0),
    Port::optional("weight1", f32::INFINITY),
    Port::optional("weight2", f32::INFINITY),
    Port::optional("weight3", f32::INFINITY),
    Port::optional("weight4", f32::INFINITY),
    Port::optional("channel", f32::INFINITY),
    Port::optional("train_freq", 100.0),
    Port::optional("train_partials", 16.0),
    Port::optional("train_chroma", 1.0),
    Port::optional("train_gain", f32::INFINITY),
    Port::optional("select", f32::INFINITY),
    Port::optional("select2", f32::INFINITY),
    Port::optional("select3", f32::INFINITY),
    Port::optional("select4", f32::INFINITY),
    Port::optional("gain_index", f32::INFINITY),
    Port::optional("start_index", f32::INFINITY),
    Port::optional("end_index", f32::INFINITY),
    Port::optional("fm_index", f32::INFINITY),
    Port::optional("channel_index", f32::INFINITY),
    Port::optional("mix_index", f32::INFINITY),
    Port::optional("fm_window_index", f32::INFINITY),
    Port::optional("rate1", 1.0),
    Port::optional("rate2", 1.0),
    Port::optional("rate3", 1.0),
    Port::optional("rate4", 1.0),
    Port::optional("freq", f32::INFINITY),
];

/// Resources and allocation limits fixed when a granular instance is built.
#[derive(Clone, PartialEq)]
pub struct GranularConfig {
    pub sources: [Vec<Arc<Table>>; 4],
    /// A bank of mono tables; each reader can select a different entry at grain birth.
    pub source_bank: Vec<Arc<Table>>,
    pub window: Option<Arc<Table>>,
    pub attack_window: Option<Arc<Table>>,
    pub decay_window: Option<Arc<Table>>,
    pub fm_window: Option<Arc<Table>>,
    pub fm_windows: Vec<Arc<Table>>,
    pub distribution_table: Option<Arc<Table>>,
    /// Eight concatenated panning curves, one per neighboring output pair.
    pub pan_law: Option<Arc<Table>>,
    /// Gain, start ratio, end ratio, FM depth, channel, and five-way source mix.
    pub masks: [Option<Arc<Table>>; MASKS],
    pub channels: usize,
    pub max_grains: usize,
    pub monitor: bool,
    pub seed: u32,
}

impl Default for GranularConfig {
    fn default() -> Self {
        Self {
            sources: [
                vec![Arc::new(Table::from_f64([0.0], None))],
                vec![],
                vec![],
                vec![],
            ],
            source_bank: vec![],
            window: None,
            attack_window: None,
            decay_window: None,
            fm_window: None,
            fm_windows: vec![],
            distribution_table: None,
            pan_law: None,
            masks: std::array::from_fn(|_| None),
            channels: 2,
            max_grains: CAPACITY,
            monitor: false,
            seed: 0,
        }
    }
}

#[derive(Clone, PartialEq)]
struct Source {
    channels: [Arc<Table>; 2],
    stereo: bool,
}
impl Source {
    fn new(channels: Vec<Arc<Table>>) -> Result<Self, String> {
        if !(1..=2).contains(&channels.len()) || channels.iter().any(|t| t.is_empty()) {
            return Err("a granular source needs one or two nonempty channel tables".into());
        }
        if channels.len() == 2
            && (channels[0].len() != channels[1].len()
                || channels[0].sample_rate() != channels[1].sample_rate())
        {
            return Err(
                "a stereo granular source needs matching channel lengths and sample rates".into(),
            );
        }
        Ok(Self {
            channels: [channels[0].clone(), channels[channels.len() - 1].clone()],
            stereo: channels.len() == 2,
        })
    }
    fn rate(&self, fallback: f32) -> f64 {
        self.channels[0].sample_rate_or(fallback) as f64
    }
    fn len(&self) -> f64 {
        self.channels[0].len() as f64
    }
}

#[derive(Clone, Copy, Default)]
struct Grain {
    position: [f64; 4],
    step: [f64; 5],
    sweep_offset: [f64; 5],
    sweep_decay: f64,
    source_ids: [usize; 4],
    weights: [f32; 5],
    gain: f32,
    pan: [f32; 8],
    stereo_balance: bool,
    fm_depth: f32,
    fm_window_id: usize,
    window_amount: f32,
    attack: f32,
    sustain: f32,
    train_phase: f64,
    train_partials: usize,
    train_chroma: f64,
    age: u32,
    length: u32,
    delay: u32,
    fraction: f64,
    birth: u64,
}

pub struct Granular {
    config: Arc<GranularConfig>,
    sources: [Option<Source>; 4],
    bank: Arc<[Source]>,
    masks: [Option<Mask>; MASKS],
    mask_indices: [usize; MASKS],
    grains: Vec<Grain>,
    sample_rate: f32,
    countdown: f64,
    clock_interval: f64,
    previous_trigger: f32,
    synced: bool,
    clock_started: bool,
    seed: u32,
    rng: u32,
    probability_rng: u32,
    distribution_rng: u32,
    distribution_index: usize,
    births: u64,
}

impl Clone for Granular {
    fn clone(&self) -> Self {
        // Vec::clone would discard reserved capacity for an idle cloud, causing
        // the first grain of a cloned voice to allocate on the audio thread.
        let mut grains = vec![Grain::default(); self.config.max_grains];
        grains.truncate(self.grains.len());
        grains.copy_from_slice(&self.grains);
        Self {
            config: self.config.clone(),
            sources: self.sources.clone(),
            bank: self.bank.clone(),
            masks: self.masks.clone(),
            mask_indices: self.mask_indices,
            grains,
            sample_rate: self.sample_rate,
            countdown: self.countdown,
            clock_interval: self.clock_interval,
            previous_trigger: self.previous_trigger,
            synced: self.synced,
            clock_started: self.clock_started,
            seed: self.seed,
            rng: self.rng,
            probability_rng: self.probability_rng,
            distribution_rng: self.distribution_rng,
            distribution_index: self.distribution_index,
            births: self.births,
        }
    }
}

fn finite(x: f32, default: f32) -> f32 {
    if x.is_finite() { x } else { default }
}
fn value(p: &[f32], index: usize) -> f32 {
    p.get(index)
        .copied()
        .unwrap_or(PORTS[index].default.unwrap())
}
fn next_random(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}
fn probability_seed(seed: u32) -> u32 {
    (seed ^ 0xa511_e9b3).max(1)
}
fn window(table: &Table, phase: f64) -> f32 {
    table.read_at_edge(
        phase.clamp(0.0, 1.0) * (table.len() - 1) as f64,
        Interp::Linear,
        Edge::Clamp,
    ) as f32
}
fn source_at<'a>(
    sources: &'a [Option<Source>; 4],
    bank: &'a [Source],
    id: usize,
) -> Option<&'a Source> {
    if id < 4 {
        sources[id].as_ref()
    } else {
        bank.get(id - 4)
    }
}

impl Granular {
    pub fn new(config: GranularConfig) -> Result<Self, String> {
        if !(1..=8).contains(&config.channels) {
            return Err("granular channels must be a whole number from 1 to 8".into());
        }
        if !(1..=MAX_GRAINS).contains(&config.max_grains) {
            return Err(format!(
                "max_grains must be a whole number from 1 to {MAX_GRAINS}"
            ));
        }
        for table in [
            &config.window,
            &config.attack_window,
            &config.decay_window,
            &config.fm_window,
        ]
        .into_iter()
        .flatten()
        .chain(config.fm_windows.iter())
        {
            if table.len() < 2 || table.data().iter().any(|v| !v.is_finite()) {
                return Err(
                    "a grain window needs one table with at least two finite values".into(),
                );
            }
        }
        if let Some(table) = &config.distribution_table
            && (table.is_empty() || table.data().iter().any(|v| !v.is_finite()))
        {
            return Err("distribution_table needs at least one finite value".into());
        }
        if let Some(table) = &config.pan_law
            && (table.len() < 16
                || !table.len().is_multiple_of(8)
                || table.data().iter().any(|v| !v.is_finite()))
        {
            return Err(
                "pan_law needs eight equal curves, each with at least two finite values".into(),
            );
        }
        let mut sources = std::array::from_fn(|_| None);
        for (out, tables) in sources.iter_mut().zip(&config.sources) {
            if !tables.is_empty() {
                *out = Some(Source::new(tables.clone())?);
            }
        }
        let bank: Arc<[Source]> = config
            .source_bank
            .iter()
            .map(|t| Source::new(vec![t.clone()]))
            .collect::<Result<Vec<_>, _>>()?
            .into();
        let mut masks = std::array::from_fn(|_| None);
        for (i, table) in config.masks.iter().enumerate() {
            if let Some(table) = table {
                masks[i] = Some(Mask::new(table.clone(), if i == 5 { 5 } else { 1 })?);
            }
        }
        let seed = config.seed.wrapping_add(0x9e3779b9).max(1);
        // Touch the reserved pages during preparation, before the audio thread pushes grains.
        let mut grains = vec![Grain::default(); config.max_grains];
        grains.clear();
        Ok(Self {
            grains,
            config: Arc::new(config),
            sources,
            bank,
            masks,
            mask_indices: [0; MASKS],
            sample_rate: 48000.0,
            countdown: 0.0,
            clock_interval: 1.0,
            previous_trigger: 0.0,
            synced: false,
            clock_started: false,
            seed,
            rng: seed,
            probability_rng: probability_seed(seed),
            distribution_rng: (seed ^ 0x63d8_3595).max(1),
            distribution_index: 0,
            births: 0,
        })
    }

    fn random(&mut self) -> f32 {
        next_random(&mut self.rng) as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    fn masks(&mut self) -> [[f32; 5]; MASKS] {
        let mut values = [[1.0; 5]; MASKS];
        values[4][0] = 0.0;
        for (i, out) in values.iter_mut().enumerate() {
            if let Some(mask) = &self.masks[i] {
                *out = mask.read(&mut self.mask_indices[i]);
            } else {
                self.mask_indices[i] = 1;
            }
        }
        values
    }

    fn route(&self, p: &[f32], masks: &[[f32; 5]; MASKS], pan: f32) -> ([f32; 8], bool) {
        let n = self.config.channels;
        let explicit = value(p, 35).is_finite()
            || self.masks[4].is_some()
            || self.config.pan_law.is_some()
            || n != 2;
        let mut result = [0.0; 8];
        if !explicit {
            let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
            result[0] = angle.cos().max(0.0);
            result[1] = angle.sin();
            return (result, true);
        }
        if n == 1 {
            result[0] = 1.0;
            return (result, false);
        }
        let default_channel = if self.masks[4].is_some() {
            0.0
        } else {
            (pan + 1.0) * 0.5 * (n - 1) as f32
        };
        let channel = (finite(value(p, 35), default_channel) + masks[4][0]).rem_euclid(n as f32);
        let first = channel.floor() as usize;
        let next = (first + 1) % n;
        let t = channel.fract();
        let gains = if let Some(table) = &self.config.pan_law {
            let len = table.len() / 8;
            let read = |phase: f32| {
                let pos = phase as f64 * (len - 1) as f64;
                let i = pos.floor() as usize;
                let f = (pos - i as f64) as f32;
                let base = first * len;
                let a = table.get(base + i);
                a + (table.get(base + (i + 1).min(len - 1)) - a) * f
            };
            [read(1.0 - t), read(t)]
        } else {
            [1.0 - t, t]
        };
        result[first] += gains[0];
        result[next] += gains[1];
        (result, false)
    }

    fn displacement(&mut self, p: &[f32]) -> f64 {
        let distribution = finite(value(p, 20), 0.0);
        let rnd = next_random(&mut self.distribution_rng);
        let density = finite(p[0], 0.0).abs();
        if density < 0.001 {
            return 0.0;
        }
        let Some(table) = &self.config.distribution_table else {
            return 0.0;
        };
        let index = if distribution < 0.0 {
            let index = self.distribution_index % table.len();
            self.distribution_index = (index + 1) % table.len();
            index
        } else {
            ((rnd as u64 * table.len() as u64) >> 32) as usize
        };
        (table.get(index) as f64 * distribution.abs() as f64 / density as f64).clamp(0.0, 10.0)
            * self.sample_rate as f64
    }

    fn spawn_at(&mut self, p: &[f32], overdue: f64) {
        let masks = self.masks();
        let duration = finite(p[1], 0.1).clamp(2.0 / self.sample_rate, 10.0);
        let length = (duration * self.sample_rate).round() as u32;
        let position = finite(p[2], 0.0);
        let positions = [
            position,
            finite(p[17], position),
            finite(value(p, 27), position),
            finite(value(p, 29), position),
        ];
        let pitches = [
            finite(value(p, 26), 0.0),
            finite(p[18], 0.0),
            finite(value(p, 28), 0.0),
            finite(value(p, 30), 0.0),
        ];
        let offset = self.random() as f64 * finite(p[4], 0.0).clamp(0.0, 10.0) as f64;
        let pitch = self.random() * finite(p[5], 0.0).clamp(0.0, 96.0);
        let rate = finite(p[3], 1.0).clamp(-64.0, 64.0) as f64 * 2.0_f64.powf(pitch as f64 / 12.0);
        let offset_pan = self.random() * finite(p[11], 0.0).clamp(0.0, 2.0);
        let pan = (finite(p[10], 0.0) + offset_pan).clamp(-1.0, 1.0);
        let (routing, stereo_balance) = self.route(p, &masks, pan);
        let mix = if self.sources[1].is_some() || value(p, 41).is_finite() {
            finite(p[14], 0.0).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mix_mask = self.masks[5].is_some();
        let weight_defaults = if mix_mask {
            [1.0; 5]
        } else {
            [
                1.0 - mix,
                mix,
                if self.sources[2].is_some() || value(p, 42).is_finite() {
                    1.0
                } else {
                    0.0
                },
                if self.sources[3].is_some() || value(p, 43).is_finite() {
                    1.0
                } else {
                    0.0
                },
                0.0,
            ]
        };
        let mut grain = Grain {
            age: 0,
            length,
            gain: finite(p[15], 0.0) * masks[0][0],
            attack: finite(p[8], 0.5).clamp(0.0, 1.0),
            sustain: finite(value(p, 21), 0.0).clamp(0.0, 1.0),
            window_amount: finite(value(p, 22), 1.0).clamp(0.0, 1.0),
            fm_depth: finite(p[13], 0.0) * masks[3][0],
            fm_window_id: if value(p, 50).is_finite() && !self.config.fm_windows.is_empty() {
                (value(p, 50) as f64)
                    .floor()
                    .rem_euclid(self.config.fm_windows.len() as f64) as usize
            } else {
                usize::MAX
            },
            pan: routing,
            stereo_balance,
            weights: [
                finite(value(p, 31), weight_defaults[0]),
                finite(value(p, 32), weight_defaults[1]),
                finite(value(p, 33), weight_defaults[2]),
                finite(value(p, 34), weight_defaults[3]),
                finite(value(p, 39), weight_defaults[4]),
            ],
            train_phase: 0.5,
            train_chroma: finite(value(p, 38), 1.0) as f64,
            birth: self.births,
            ..Grain::default()
        };
        let start = 2.0_f64.powf(finite(value(p, 24), 0.0).clamp(-96.0, 96.0) as f64 / 12.0)
            * masks[1][0] as f64;
        let end_pitch = finite(value(p, 25), finite(value(p, 24), 0.0) + finite(p[9], 0.0));
        let end = 2.0_f64.powf(end_pitch.clamp(-192.0, 192.0) as f64 / 12.0) * masks[2][0] as f64;
        let shape = value(p, 23);
        let (decay, offset_sweep) = if !shape.is_finite() && start * end > 0.0 {
            ((end / start).powf(1.0 / length as f64), 0.0)
        } else if shape.is_finite() && shape <= 0.001 {
            (1.0, 0.0)
        } else if shape.is_finite() && shape >= 0.999 {
            (0.0, end)
        } else if !shape.is_finite() || (shape - 0.5).abs() < 1e-6 {
            (1.0, (end - start) / length as f64)
        } else {
            let total = ((1.0 - shape as f64) / shape as f64).powi(2);
            let decay = total.powf(1.0 / length as f64);
            (decay, (end - start * total) / (1.0 - total) * (1.0 - decay))
        };
        grain.sweep_decay = decay;
        let initial = if shape.is_finite() && shape >= 0.999 {
            end
        } else {
            start
        };
        for i in 0..4 {
            grain.weights[i] *= masks[5][i];
            let select = value(p, 40 + i);
            let id = if select.is_finite() && !self.bank.is_empty() {
                4 + (select as f64).floor().rem_euclid(self.bank.len() as f64) as usize
            } else {
                i
            };
            grain.source_ids[i] = id;
            let Some(source) = source_at(&self.sources, &self.bank, id) else {
                grain.weights[i] = 0.0;
                continue;
            };
            let sr = source.rate(self.sample_rate);
            let frequency = value(p, 55);
            let frames_per_second = if frequency.is_finite() {
                frequency as f64 * source.len()
            } else {
                sr
            };
            let base = rate
                * finite(value(p, 51 + i), 1.0) as f64
                * 2.0_f64.powf(pitches[i].clamp(-96.0, 96.0) as f64 / 12.0)
                * frames_per_second
                / self.sample_rate as f64;
            grain.position[i] = (positions[i] as f64).rem_euclid(1.0) * source.len() + offset * sr;
            grain.position[i] = grain.position[i].rem_euclid(source.len());
            grain.step[i] = base * initial;
            grain.sweep_offset[i] = base * offset_sweep;
        }
        grain.weights[4] *= masks[5][4];
        let train_base = finite(value(p, 36), 100.0)
            .clamp(-self.sample_rate * 0.5, self.sample_rate * 0.5) as f64
            / self.sample_rate as f64;
        grain.step[4] = train_base * initial;
        grain.sweep_offset[4] = train_base * offset_sweep;
        let max_frequency = (train_base.abs() * start.abs().max(end.abs())).max(f64::MIN_POSITIVE);
        grain.train_partials = (finite(value(p, 37), 16.0).abs() as usize)
            .max(1)
            .min((0.5 / max_frequency).min(i32::MAX as f64) as usize);
        self.births = self.births.wrapping_add(1);
        let delay = self.displacement(p) - overdue;
        if delay > 0.0 {
            grain.delay = delay.ceil() as u32;
            grain.fraction = delay.ceil() - delay;
        } else {
            grain.fraction = (-delay).min(1.0);
        }
        // A masked candidate still consumes masks and random choices, but cannot steal a voice.
        let chance = next_random(&mut self.probability_rng) as f64 / 4294967296.0;
        if chance >= finite(p[16], 0.0).clamp(0.0, 1.0) as f64 || grain.gain == 0.0 {
            return;
        }
        for i in 0..4 {
            grain.position[i] += grain.fraction * grain.step[i];
        }
        grain.train_phase += grain.fraction * grain.step[4];
        if self.grains.len() < self.config.max_grains {
            self.grains.push(grain); // the entire pool was reserved before processing
        } else {
            let slot = self
                .grains
                .iter()
                .enumerate()
                .min_by_key(|(_, g)| g.birth)
                .unwrap()
                .0;
            self.grains[slot] = grain;
        }
    }

    fn envelope(&self, g: &Grain, phase: f64) -> f32 {
        let attack_end = (1.0 - g.sustain) * g.attack;
        let decay_start = attack_end + g.sustain;
        let primary = if phase < attack_end as f64 && attack_end > 0.0 {
            let t = phase / attack_end as f64;
            if let Some(table) = &self.config.attack_window {
                window(table, t)
            } else if self.config.window.is_some() {
                1.0
            } else {
                (0.5 - 0.5 * (std::f64::consts::PI * t).cos()) as f32
            }
        } else if phase >= decay_start as f64 && decay_start < 1.0 {
            let t = (phase - decay_start as f64) / (1.0 - decay_start as f64);
            if let Some(table) = &self.config.decay_window {
                window(table, t)
            } else if self.config.window.is_some() {
                1.0
            } else {
                (0.5 + 0.5 * (std::f64::consts::PI * t).cos()) as f32
            }
        } else {
            self.config
                .attack_window
                .as_ref()
                .map_or(1.0, |t| t.get(t.len() - 1))
        };
        primary
            * self
                .config
                .window
                .as_ref()
                .map_or(1.0, |t| 1.0 + g.window_amount * (window(t, phase) - 1.0))
    }

    fn frame(&mut self, p: &[f32], scheduling: bool) -> [f32; 16] {
        for (i, cursor) in self.mask_indices.iter_mut().enumerate() {
            let input = value(p, 44 + i);
            if input.is_finite() {
                *cursor = input.max(0.0) as usize;
            }
        }
        let density = finite(p[0], 0.0).abs().min(self.sample_rate);
        if !self.clock_started {
            self.countdown = if density > 0.0 { 0.0 } else { 1.0 };
            self.clock_started = true;
        }
        let sync = finite(value(p, 19), 0.0);
        let held = sync >= 1.0;
        let hard = held && !self.synced;
        if hard {
            self.countdown = 0.0;
        } else if !held && sync != 0.0 {
            self.countdown = (self.countdown - sync as f64 * self.clock_interval)
                .clamp(0.0, self.clock_interval);
        }
        self.synced = held;
        let clock = scheduling
            && ((!held && self.countdown <= 0.0 && (density > 0.0 || sync != 0.0)) || hard);
        let trigger = finite(p[7], 0.0);
        let external = scheduling && trigger > 0.0 && self.previous_trigger <= 0.0;
        self.previous_trigger = trigger;
        let mut output = [0.0; 16];
        if clock || external {
            let overdue = if clock && !hard && sync == 0.0 && density > 0.0 {
                (-self.countdown * self.sample_rate as f64 / density as f64).clamp(0.0, 1.0)
            } else {
                0.0
            };
            self.spawn_at(p, overdue);
            output[self.config.channels] = 1.0;
        }
        if clock {
            let jitter = finite(p[6], 0.0).clamp(0.0, 1.0);
            self.clock_interval = (1.0 + self.random() as f64 * jitter as f64).max(0.01);
            self.countdown += self.clock_interval;
        }
        output[self.config.channels + 1] =
            (1.0 - self.countdown / self.clock_interval).rem_euclid(1.0) as f32;
        for (i, cursor) in self.mask_indices.iter().enumerate() {
            output[self.config.channels + 2 + i] = *cursor as f32;
        }
        if density > 0.0 && !held {
            self.countdown -= density as f64 / self.sample_rate as f64;
        }
        // Temporarily take the fixed pool to borrow immutable resources without per-sample allocation.
        let mut grains = std::mem::take(&mut self.grains);
        for g in grains.iter_mut().filter(|g| g.age < g.length) {
            if g.delay > 0 {
                if scheduling {
                    g.delay -= 1;
                } else {
                    g.age = g.length;
                }
                continue;
            }
            let phase = ((g.age as f64 + g.fraction) / (g.length - 1) as f64).min(1.0);
            let level = self.envelope(g, phase) * g.gain;
            let fm = finite(p[12], 0.0).clamp(-64.0, 64.0) as f64
                * g.fm_depth as f64
                * self
                    .config
                    .fm_windows
                    .get(g.fm_window_id)
                    .or(self.config.fm_window.as_ref())
                    .map_or(1.0, |t| window(t, phase)) as f64;
            for i in 0..4 {
                if g.weights[i] == 0.0 {
                    continue;
                }
                let Some(source) = source_at(&self.sources, &self.bank, g.source_ids[i]) else {
                    continue;
                };
                let left = source.channels[0].read_at(g.position[i], Interp::Cubic) as f32;
                if g.stereo_balance {
                    let right = if source.stereo {
                        source.channels[1].read_at(g.position[i], Interp::Cubic) as f32
                    } else {
                        left
                    };
                    let gain = level
                        * g.weights[i]
                        * if source.stereo {
                            std::f32::consts::SQRT_2
                        } else {
                            1.0
                        };
                    output[0] += left * gain * g.pan[0];
                    output[1] += right * gain * g.pan[1];
                } else {
                    let mono = if source.stereo {
                        (left + source.channels[1].read_at(g.position[i], Interp::Cubic) as f32)
                            * 0.5
                    } else {
                        left
                    };
                    for (out, pan) in output[..self.config.channels].iter_mut().zip(g.pan) {
                        *out += mono * level * g.weights[i] * pan;
                    }
                }
                g.position[i] = (g.position[i] + g.step[i] * (1.0 + fm)).rem_euclid(source.len());
                g.step[i] = g.step[i] * g.sweep_decay + g.sweep_offset[i];
            }
            if g.weights[4] != 0.0 && g.train_partials > 0 {
                let train = trainlet::sample(g.train_phase, g.train_partials, g.train_chroma)
                    * level
                    * g.weights[4];
                for (out, pan) in output[..self.config.channels].iter_mut().zip(g.pan) {
                    *out += train * pan;
                }
                g.train_phase = (g.train_phase + g.step[4] * (1.0 + fm)).rem_euclid(1.0);
                g.step[4] = g.step[4] * g.sweep_decay + g.sweep_offset[4];
            }
            g.age += 1;
        }
        grains.retain(|g| g.age < g.length);
        self.grains = grains;
        output
    }

    #[cfg(test)]
    fn step(&mut self, p: &[f32], scheduling: bool) -> [f32; 2] {
        let out = self.frame(p, scheduling);
        [out[0], out[1]]
    }
    #[cfg(test)]
    fn spawn(&mut self, p: &[f32]) {
        self.spawn_at(p, 0.0);
    }
}

impl Opcode for Granular {
    fn name(&self) -> &str {
        "granular"
    }
    fn ports(&self) -> &[Port] {
        PORTS
    }
    fn has_process_one(&self) -> bool {
        self.outputs() == 1
    }
    fn outputs(&self) -> usize {
        self.config.channels
            + if self.config.monitor {
                GRANULAR_MONITOR_FIELDS.len()
            } else {
                0
            }
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }
    fn reset(&mut self) {
        self.grains.clear();
        self.countdown = 0.0;
        self.clock_interval = 1.0;
        self.previous_trigger = 0.0;
        self.synced = false;
        self.clock_started = false;
        self.rng = self.seed;
        self.probability_rng = probability_seed(self.seed);
        self.distribution_rng = (self.seed ^ 0x63d8_3595).max(1);
        self.mask_indices = [0; MASKS];
        self.distribution_index = 0;
        self.births = 0;
    }
    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut fresh = self.clone();
        fresh.reset();
        Box::new(fresh)
    }
    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.config == old.config
        {
            std::mem::swap(&mut self.grains, &mut old.grains);
            self.countdown = old.countdown;
            self.clock_interval = old.clock_interval;
            self.previous_trigger = old.previous_trigger;
            self.synced = old.synced;
            self.clock_started = old.clock_started;
            self.rng = old.rng;
            self.probability_rng = old.probability_rng;
            self.distribution_rng = old.distribution_rng;
            self.mask_indices = old.mask_indices;
            self.distribution_index = old.distribution_index;
            self.births = old.births;
        }
    }
    fn process_one(&mut self, ctx: &ProcessCtx, inputs: &[f32]) -> f32 {
        self.frame(inputs, ctx.gate || ctx.envelopes_active)[0]
    }
    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        let mut p: [f32; PORTS.len()] = std::array::from_fn(|i| PORTS[i].default.unwrap());
        for (i, out) in out.iter_mut().enumerate() {
            for (v, input) in p.iter_mut().zip(inputs) {
                *v = input[i];
            }
            *out = self.frame(&p, ctx.gate || ctx.envelopes_active)[0];
        }
    }
    fn process_outputs(
        &mut self,
        ctx: &ProcessCtx,
        inputs: &[&[f32]],
        outs: &mut [Vec<f32>],
        frames: usize,
    ) {
        let mut p: [f32; PORTS.len()] = std::array::from_fn(|i| PORTS[i].default.unwrap());
        for i in 0..frames {
            for (v, input) in p.iter_mut().zip(inputs) {
                *v = input[i];
            }
            let sample = self.frame(&p, ctx.gate || ctx.envelopes_active);
            for (out, sample) in outs.iter_mut().zip(sample) {
                out[i] = sample;
            }
        }
    }
}
