//! The language's view of built-in opcodes: names, argument order, and the unit
//! each argument expects. Argument names match the engine's port names. User
//! functions are compiled where they are called (see `lower` and `kernel`).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};

use niminal_engine::Opcode;
pub use niminal_engine::ops::Wave;
use niminal_engine::ops::{
    Allpass, Bitcrush, Chorus, Clip, Comb, Compressor, DcBlock, Delay, Distortion, Eq, EqMode,
    FilterMode, FinishWhen, Fn1, Fold, Follow, Formant, Func, Gain, Gate, Impulse, Lag, Limiter,
    Noise, NoiseColor, Osc, Phaser, PitchShift, Resonator, Reverb, SampleHold, Svf, Wavetable,
};

use crate::unit::Unit;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// A waveform name such as `saw`.
    Wave,
    /// A signal or value of exactly this unit.
    Signal(Unit),
    /// A gain: decibels, or a plain linear factor.
    Gain,
    /// A channel layout such as `stereo`.
    Layout,
}

pub struct ParamSpec {
    pub name: String,
    pub kind: Kind,
    pub required: bool,
    /// False for settings that shape the opcode when it is built (such as a
    /// delay's `max`) rather than signals wired into it.
    pub wired: bool,
}

fn req(name: &str, kind: Kind) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        kind,
        required: true,
        wired: true,
    }
}

fn opt(name: &str, kind: Kind) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        kind,
        required: false,
        wired: true,
    }
}

fn setting(name: &str, kind: Kind) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        kind,
        required: false,
        wired: false,
    }
}

/// What an oscillator plays: a built-in waveform, one cycle of a table, or a morphing bank.
#[derive(Clone)]
pub enum WaveArg {
    Builtin(Wave),
    Table(Arc<Wavetable>),
    /// Several band-limited tables morphable with `position:`.
    Bank(Arc<[Arc<Wavetable>]>),
}

/// The longest table an oscillator accepts.
const MAX_TABLE: usize = 1 << 16;

impl WaveArg {
    /// One cycle of a wave, as given in a program.
    pub fn table(cycle: &[f64]) -> Result<Self, String> {
        if cycle.is_empty() {
            return Err("a table can't be empty".into());
        }
        if cycle.len() > MAX_TABLE {
            return Err(format!(
                "a table to play can have at most {MAX_TABLE} entries"
            ));
        }
        Ok(WaveArg::Table(cached_wavetable(cycle)))
    }

    pub fn from_table(table: &niminal_engine::Table) -> Result<Self, String> {
        if table.is_empty() {
            return Err("a table can't be empty".into());
        }
        if table.len() > MAX_TABLE {
            return Err(format!(
                "a table to play can have at most {MAX_TABLE} entries"
            ));
        }
        Ok(WaveArg::Table(cached_wavetable(&table.as_f64())))
    }

    pub fn bank(tables: &[Arc<niminal_engine::Table>]) -> Result<Self, String> {
        if tables.len() < 2 {
            return Err("a wavetable bank needs at least two tables".into());
        }
        let waves: Result<Vec<_>, _> = tables
            .iter()
            .map(|t| {
                if t.is_empty() {
                    return Err("a table can't be empty".into());
                }
                if t.len() > MAX_TABLE {
                    return Err(format!(
                        "a table to play can have at most {MAX_TABLE} entries"
                    ));
                }
                Ok(cached_wavetable(&t.as_f64()))
            })
            .collect();
        Ok(WaveArg::Bank(waves?.into()))
    }
}

/// Tables are shared while any program still plays them, so the same cycle is analysed once.
fn cached_wavetable(cycle: &[f64]) -> Arc<Wavetable> {
    static MADE: LazyLock<Mutex<HashMap<Vec<u64>, Weak<Wavetable>>>> =
        LazyLock::new(Mutex::default);
    let key: Vec<u64> = cycle.iter().map(|v| v.to_bits()).collect();
    let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(table) = made.get(&key).and_then(Weak::upgrade) {
        return table;
    }
    made.retain(|_, table| table.strong_count() > 0);
    let table = Arc::new(Wavetable::from_cycle(cycle));
    made.insert(key, Arc::downgrade(&table));
    table
}

/// What is known at compile time when an opcode is built.
#[derive(Default)]
pub struct BuildArgs {
    pub wave: Option<WaveArg>,
    /// Which channel of a multichannel signal this copy of the opcode serves.
    pub channel: usize,
    /// Tells apart the copies of an opcode in one instrument, for those that vary by it.
    pub salt: u32,
    /// Arguments whose value is a compile-time constant, in engine units.
    pub consts: HashMap<String, f64>,
}

type BuildFn = fn(&BuildArgs) -> Result<Box<dyn Opcode>, String>;

/// `seed:` of a noise source, a whole number.
fn seed_of(a: &BuildArgs) -> Option<u32> {
    a.consts.get("seed").map(|s| s.round() as i64 as u32)
}

/// The longest delay line a program may ask for.
const MAX_DELAY_SECONDS: f64 = 30.0;

pub enum Build {
    Builtin(BuildFn),
    /// `pan`: one output per speaker of the master layout.
    Pan,
    /// `to_layout`: converts between layouts.
    ToLayout,
    /// `convolve`: the compiler loads the IR buffer and builds the IrSpectrum.
    Convolve,
    /// `fft`: the compiler creates the SpectralChain shared with downstream ops.
    Fft,
    /// `ifft`: the compiler wires the SpectralChain from the upstream FFT.
    Ifft,
    /// `freeze`: the compiler wires the SpectralChain from the upstream FFT.
    SpectralFreeze,
    /// `spectral_delay`: per-bin delay with spread and feedback, compiler wires SpectralChain.
    SpectralDelay,
}

pub struct OpSpec {
    pub name: String,
    /// What the opcode does, for hover and completion.
    pub doc: String,
    pub params: Vec<ParamSpec>,
    /// How many leading arguments may be given without a name: the leading
    /// required ones. The receiver of a method call counts as the first.
    pub positional: usize,
    pub build: Build,
}

impl OpSpec {
    fn new(name: &str, doc: &str, params: Vec<ParamSpec>, build: Build) -> Self {
        OpSpec {
            name: name.into(),
            doc: doc.into(),
            positional: crate::funcs::positional(params.iter().map(|p| p.required)),
            params,
            build,
        }
    }

    /// `Ok(None)` for an opcode that isn't a node of its own (`pan`, `to_layout`).
    pub fn instantiate(&self, args: &BuildArgs) -> Result<Option<Box<dyn Opcode>>, String> {
        match &self.build {
            Build::Builtin(f) => f(args).map(Some),
            Build::Pan | Build::ToLayout => Ok(None),
            Build::Convolve
            | Build::Fft
            | Build::Ifft
            | Build::SpectralFreeze
            | Build::SpectralDelay => Ok(None),
        }
    }
}

const SIGNAL: Kind = Kind::Signal(Unit::Num);

fn filter_params() -> Vec<ParamSpec> {
    vec![
        req("x", SIGNAL),
        req("cutoff", Kind::Signal(Unit::Hz)),
        opt("res", SIGNAL),
    ]
}

fn filter(name: &str, doc: &str, build: BuildFn) -> OpSpec {
    OpSpec::new(name, doc, filter_params(), Build::Builtin(build))
}

fn math(name: &str, doc: &str, build: BuildFn) -> OpSpec {
    OpSpec::new(name, doc, vec![req("x", SIGNAL)], Build::Builtin(build))
}

pub struct Registry {
    ops: Vec<OpSpec>,
}

impl Registry {
    pub fn builtin() -> Self {
        let ops = vec![
            OpSpec::new(
                "finish_when",
                "Silence this layer permanently when done becomes positive. After release, the voice waits for every layer and downstream tail to finish.",
                vec![req("x", SIGNAL), req("done", SIGNAL)],
                Build::Builtin(|_| Ok(Box::new(FinishWhen::default()))),
            ),
            OpSpec::new(
                "osc",
                "An oscillator: `osc(saw, 440hz)`. The waveform is sine, saw, square or tri, a table (`harmonics([...])`), or a `bank([...])` of tables; `phase_mod` bends the phase for FM; `position` morphs a bank.",
                vec![
                    req("wave", Kind::Wave),
                    req("freq", Kind::Signal(Unit::Hz)),
                    opt("phase_mod", SIGNAL),
                    opt("position", SIGNAL),
                ],
                Build::Builtin(|a| {
                    Ok(Box::new(match a.wave.clone().expect("osc has a wave") {
                        WaveArg::Builtin(wave) => Osc::new(wave),
                        WaveArg::Table(table) => Osc::with_table(table),
                        WaveArg::Bank(bank) => Osc::with_bank(bank),
                    }))
                }),
            ),
            filter(
                "lpf",
                "A low-pass filter: lets frequencies below `cutoff` through. `res` adds resonance.",
                |_| Ok(Box::new(Svf::new(FilterMode::Low))),
            ),
            filter(
                "hpf",
                "A high-pass filter: lets frequencies above `cutoff` through. `res` adds resonance.",
                |_| Ok(Box::new(Svf::new(FilterMode::High))),
            ),
            filter(
                "bpf",
                "A band-pass filter: lets frequencies around `cutoff` through. `res` narrows the band.",
                |_| Ok(Box::new(Svf::new(FilterMode::Band))),
            ),
            filter(
                "notch",
                "A notch filter: removes frequencies around `cutoff`. `res` narrows the notch.",
                |_| Ok(Box::new(Svf::new(FilterMode::Notch))),
            ),
            OpSpec::new(
                "gain",
                "Scales a signal by a gain in db or a plain factor.",
                vec![req("x", SIGNAL), req("gain", Kind::Gain)],
                Build::Builtin(|_| Ok(Box::new(Gain))),
            ),
            OpSpec::new(
                "delay",
                "A delay line. `time` is how long to wait, `feedback` feeds the output back in. A changing `time` needs `max:` to size the line (at most 30 seconds).",
                vec![
                    req("x", SIGNAL),
                    req("time", Kind::Signal(Unit::Time)),
                    opt("feedback", SIGNAL),
                    setting("max", Kind::Signal(Unit::Time)),
                ],
                Build::Builtin(|a| {
                    let max = a.consts.get("max").or_else(|| a.consts.get("time")).copied().ok_or_else(|| {
                        "`delay` needs a constant `time`, or a `max:` that bounds a changing one".to_string()
                    })?;
                    if max > MAX_DELAY_SECONDS {
                        return Err(format!(
                            "a delay can be at most {MAX_DELAY_SECONDS} seconds long"
                        ));
                    }
                    Ok(Box::new(Delay::new(max.max(0.0) as f32)))
                }),
            ),
            OpSpec::new(
                "reverb",
                "A reverb: `room` sets its size and `damp` how quickly the highs die away.",
                vec![req("x", SIGNAL), opt("room", SIGNAL), opt("damp", SIGNAL)],
                Build::Builtin(|a| Ok(Box::new(Reverb::for_channel(a.channel)))),
            ),
            OpSpec::new(
                "noise",
                "White noise in -1..1: `noise()`, or `noise() * env[1 5ms 0]` for a click. Each source and each note has its own noise, the same on every run; `seed:` makes it the same for every note.",
                vec![setting("seed", SIGNAL)],
                Build::Builtin(|a| Ok(Box::new(Noise::new(NoiseColor::White, seed_of(a), a.salt)))),
            ),
            OpSpec::new(
                "pinknoise",
                "Pink noise (1/f). Quieter than `noise`, useful for hats and textures. `seed:` works as for `noise`.",
                vec![setting("seed", SIGNAL)],
                Build::Builtin(|a| Ok(Box::new(Noise::new(NoiseColor::Pink, seed_of(a), a.salt)))),
            ),
            OpSpec::new(
                "compress",
                "A compressor. `threshold` and `makeup` take db or a plain factor; `ratio`, `attack` and `release` shape the envelope. Pass `sidechain:` to key off another signal (for ducking).",
                vec![
                    req("x", SIGNAL),
                    opt("threshold", Kind::Gain),
                    opt("ratio", SIGNAL),
                    opt("attack", Kind::Signal(Unit::Time)),
                    opt("release", Kind::Signal(Unit::Time)),
                    opt("makeup", Kind::Gain),
                    opt("sidechain", SIGNAL),
                ],
                Build::Builtin(|_| Ok(Box::new(Compressor::default()))),
            ),
            OpSpec::new(
                "wavefold",
                "Wavefolds a signal: louder input folds back and adds harmonics. `drive` is how hard it folds (1 at ±1).",
                vec![req("x", SIGNAL), opt("drive", SIGNAL)],
                Build::Builtin(|_| Ok(Box::new(Fold))),
            ),
            OpSpec::new(
                "clip",
                "Hard-clips a signal to ±`level` (a plain factor, default 1).",
                vec![req("x", SIGNAL), opt("level", SIGNAL)],
                Build::Builtin(|_| Ok(Box::new(Clip))),
            ),
            OpSpec::new(
                "chorus",
                "A chorus (or flanger, with more `depth` and less `time`). `rate` is the LFO in hz; `mix` blends wet with dry.",
                vec![
                    req("x", SIGNAL),
                    opt("rate", Kind::Signal(Unit::Hz)),
                    opt("depth", SIGNAL),
                    opt("time", Kind::Signal(Unit::Time)),
                    opt("mix", SIGNAL),
                ],
                Build::Builtin(|_| Ok(Box::new(Chorus::default()))),
            ),
            OpSpec::new(
                "bitcrush",
                "Reduces sample rate and bit depth. `rate` is how often a new sample is taken (hz); `bits` is the bit depth.",
                vec![
                    req("x", SIGNAL),
                    opt("rate", Kind::Signal(Unit::Hz)),
                    opt("bits", SIGNAL),
                ],
                Build::Builtin(|_| Ok(Box::new(Bitcrush::default()))),
            ),
            OpSpec::new(
                "pan",
                "Places a signal in the master layout. `azimuth` is in degrees (0 is straight ahead, positive is right); `spread` widens it.",
                vec![
                    req("x", SIGNAL),
                    opt("azimuth", Kind::Signal(Unit::Angle)),
                    opt("spread", SIGNAL),
                ],
                Build::Pan,
            ),
            OpSpec::new(
                "to_layout",
                "Converts a signal to a channel layout such as `stereo`.",
                vec![req("x", SIGNAL), req("layout", Kind::Layout)],
                Build::ToLayout,
            ),
            math("sin", "Sine of a signal (radians).", |_| {
                Ok(Box::new(Func(Fn1::Sin)))
            }),
            math("cos", "Cosine of a signal (radians).", |_| {
                Ok(Box::new(Func(Fn1::Cos)))
            }),
            math(
                "tanh",
                "Soft-clips a signal into -1..1; a classic saturator.",
                |_| Ok(Box::new(Func(Fn1::Tanh))),
            ),
            math("exp", "e raised to a signal.", |_| {
                Ok(Box::new(Func(Fn1::Exp)))
            }),
            math("abs", "Absolute value of a signal.", |_| {
                Ok(Box::new(Func(Fn1::Abs)))
            }),
            math("tan", "Tangent of a signal (radians).", |_| {
                Ok(Box::new(Func(Fn1::Tan)))
            }),
            math("floor", "The whole number at or below a signal.", |_| {
                Ok(Box::new(Func(Fn1::Floor)))
            }),
            math("ceil", "The whole number at or above a signal.", |_| {
                Ok(Box::new(Func(Fn1::Ceil)))
            }),
            math(
                "sqrt",
                "Square root of a signal (0 for a negative one).",
                |_| Ok(Box::new(Func(Fn1::Sqrt))),
            ),
            math("log", "Natural logarithm of a signal.", |_| {
                Ok(Box::new(Func(Fn1::Log)))
            }),
            math(
                "sign",
                "1 for a positive signal, -1 for a negative one, 0 for zero.",
                |_| Ok(Box::new(Func(Fn1::Sign))),
            ),
            math(
                "fract",
                "The part of a signal after the point: `x - floor(x)`.",
                |_| Ok(Box::new(Func(Fn1::Fract))),
            ),
            // --- new opcodes ---
            OpSpec::new(
                "lag",
                "Smooths a signal toward its target: `x.lag(10ms)` slides with a 60dB convergence time. Stops parameter changes from clicking.",
                vec![req("x", SIGNAL), req("time", Kind::Signal(Unit::Time))],
                Build::Builtin(|_| Ok(Box::new(Lag::default()))),
            ),
            OpSpec::new(
                "comb",
                "A comb filter. `feedback` for tuned resonance and Karplus-Strong; `feedforward` for a comb notch/peak. `damp` lowpasses the feedback path.",
                vec![
                    req("x", SIGNAL),
                    req("time", Kind::Signal(Unit::Time)),
                    opt("feedback", SIGNAL),
                    opt("feedforward", SIGNAL),
                    opt("damp", SIGNAL),
                    setting("max", Kind::Signal(Unit::Time)),
                ],
                Build::Builtin(|a| {
                    let max = a
                        .consts
                        .get("max")
                        .or_else(|| a.consts.get("time"))
                        .copied()
                        .ok_or_else(|| {
                            "`comb` needs a constant `time`, or a `max:` that bounds a changing one"
                                .to_string()
                        })?;
                    if max > MAX_DELAY_SECONDS {
                        return Err(format!(
                            "a comb can be at most {MAX_DELAY_SECONDS} seconds long"
                        ));
                    }
                    Ok(Box::new(Comb::new(max.max(0.0) as f32)))
                }),
            ),
            OpSpec::new(
                "allpass",
                "A Schroeder allpass delay: passes all frequencies at equal level but scrambles their phase. For reverb diffusion and custom reverb design.",
                vec![
                    req("x", SIGNAL),
                    req("time", Kind::Signal(Unit::Time)),
                    opt("feedback", SIGNAL),
                    setting("max", Kind::Signal(Unit::Time)),
                ],
                Build::Builtin(|a| {
                    let max = a.consts.get("max").or_else(|| a.consts.get("time")).copied().ok_or_else(|| {
                        "`allpass` needs a constant `time`, or a `max:` that bounds a changing one".to_string()
                    })?;
                    if max > MAX_DELAY_SECONDS {
                        return Err(format!(
                            "an allpass can be at most {MAX_DELAY_SECONDS} seconds long"
                        ));
                    }
                    Ok(Box::new(Allpass::new(max.max(0.0) as f32)))
                }),
            ),
            OpSpec::new(
                "low_shelf",
                "A low shelf EQ: boosts or cuts frequencies below `cutoff`. `gain` is the boost/cut as dB or a plain factor; `q` shapes the slope.",
                vec![
                    req("x", SIGNAL),
                    req("cutoff", Kind::Signal(Unit::Hz)),
                    opt("gain", Kind::Gain),
                    opt("q", SIGNAL),
                ],
                Build::Builtin(|_| Ok(Box::new(Eq::new(EqMode::LowShelf)))),
            ),
            OpSpec::new(
                "high_shelf",
                "A high shelf EQ: boosts or cuts frequencies above `cutoff`. `gain` is the boost/cut as dB or a plain factor; `q` shapes the slope.",
                vec![
                    req("x", SIGNAL),
                    req("cutoff", Kind::Signal(Unit::Hz)),
                    opt("gain", Kind::Gain),
                    opt("q", SIGNAL),
                ],
                Build::Builtin(|_| Ok(Box::new(Eq::new(EqMode::HighShelf)))),
            ),
            OpSpec::new(
                "peak_eq",
                "A parametric peaking EQ: boosts or cuts a band around `cutoff`. `gain` is the boost/cut; `q` narrows the band (default 1).",
                vec![
                    req("x", SIGNAL),
                    req("cutoff", Kind::Signal(Unit::Hz)),
                    opt("gain", Kind::Gain),
                    opt("q", SIGNAL),
                ],
                Build::Builtin(|_| Ok(Box::new(Eq::new(EqMode::Peak)))),
            ),
            OpSpec::new(
                "limiter",
                "A brickwall lookahead limiter. `ceiling` is the maximum output level (dB or a plain factor); `release` controls recovery time.",
                vec![
                    req("x", SIGNAL),
                    opt("ceiling", Kind::Gain),
                    opt("release", Kind::Signal(Unit::Time)),
                ],
                Build::Builtin(|_| Ok(Box::new(Limiter::default()))),
            ),
            OpSpec::new(
                "gate",
                "A noise gate: silences the signal below `threshold`. `attack` and `release` shape the opening; `range` sets attenuation when closed (0 = silence).",
                vec![
                    req("x", SIGNAL),
                    opt("threshold", Kind::Gain),
                    opt("attack", Kind::Signal(Unit::Time)),
                    opt("release", Kind::Signal(Unit::Time)),
                    opt("range", SIGNAL),
                ],
                Build::Builtin(|_| Ok(Box::new(Gate::default()))),
            ),
            OpSpec::new(
                "sample_hold",
                "Captures `x` on each positive edge of `trigger` and holds it: `noise().sample_hold(impulse(4hz))` for stepped random.",
                vec![req("x", SIGNAL), req("trigger", SIGNAL)],
                Build::Builtin(|_| Ok(Box::new(SampleHold::default()))),
            ),
            OpSpec::new(
                "impulse",
                "A single-sample spike at `freq` hz: `impulse(4hz)`. The first sample is always a trigger. Use it to drive `sample_hold` or `reset_on`.",
                vec![req("freq", Kind::Signal(Unit::Hz))],
                Build::Builtin(|_| Ok(Box::new(Impulse::default()))),
            ),
            math(
                "dc_block",
                "Removes DC offset from a signal without touching anything audible.",
                |_| Ok(Box::new(DcBlock::default())),
            ),
            OpSpec::new(
                "follow",
                "Nodes the amplitude envelope of a signal. Use it for sidechain effects, reactive visuals, and ducking.",
                vec![
                    req("x", SIGNAL),
                    opt("attack", Kind::Signal(Unit::Time)),
                    opt("release", Kind::Signal(Unit::Time)),
                ],
                Build::Builtin(|_| Ok(Box::new(Follow::default()))),
            ),
            OpSpec::new(
                "phaser",
                "A phaser: allpass stages swept by an LFO. `rate` is the sweep speed, `depth` the range, `feedback` feeds the chain back, `mix` blends wet with dry.",
                vec![
                    req("x", SIGNAL),
                    opt("rate", Kind::Signal(Unit::Hz)),
                    opt("depth", SIGNAL),
                    opt("center", Kind::Signal(Unit::Hz)),
                    opt("feedback", SIGNAL),
                    opt("mix", SIGNAL),
                ],
                Build::Builtin(|_| Ok(Box::new(Phaser::default()))),
            ),
            OpSpec::new(
                "pitch_shift",
                "Real-time pitch shifting. `shift` is in semitones; `window` controls grain size (longer = smoother, more latency).",
                vec![
                    req("x", SIGNAL),
                    opt("shift", SIGNAL),
                    opt("window", Kind::Signal(Unit::Time)),
                ],
                Build::Builtin(|_| Ok(Box::new(PitchShift::default()))),
            ),
            OpSpec::new(
                "formant",
                "Vowel formant filter. `vowel` morphs through A(0) E(0.25) I(0.5) O(0.75) U(1): `saw.formant(vowel: lfo)`.",
                vec![req("x", SIGNAL), opt("vowel", SIGNAL)],
                Build::Builtin(|_| Ok(Box::new(Formant::default()))),
            ),
            OpSpec::new(
                "distortion",
                "Tube-style asymmetric distortion. `drive` pushes the signal; `tone` tilts the EQ (0 dark, 1 bright); `mix` blends.",
                vec![
                    req("x", SIGNAL),
                    opt("drive", SIGNAL),
                    opt("tone", SIGNAL),
                    opt("mix", SIGNAL),
                ],
                Build::Builtin(|_| Ok(Box::new(Distortion::default()))),
            ),
            OpSpec::new(
                "resonator",
                "A tuned resonator: rings at `freq` for `decay` seconds. Stack several for modal synthesis of bells and bars.",
                vec![
                    req("x", SIGNAL),
                    req("freq", Kind::Signal(Unit::Hz)),
                    opt("decay", Kind::Signal(Unit::Time)),
                ],
                Build::Builtin(|_| Ok(Box::new(Resonator::default()))),
            ),
            OpSpec::new(
                "convolve",
                "Convolution with an impulse response buffer: `x.convolve(ir)`. Partitioned for real-time use; `mix` blends wet with dry. Latency is one block.",
                vec![req("x", SIGNAL), opt("mix", SIGNAL)],
                Build::Convolve,
            ),
            OpSpec::new(
                "fft",
                "Forward STFT: `x.fft(1024)`. Analyses the signal into spectral bins for downstream spectral ops. The output is passed to `ifft` or spectral ops like `freeze`. Size must be a power of two from twice the block size (128 at the default) to 8192.",
                vec![req("x", SIGNAL), setting("size", SIGNAL)],
                Build::Fft,
            ),
            OpSpec::new(
                "ifft",
                "Inverse STFT: resynthesises a spectral chain back to audio. `chain.ifft()` where `chain` is the output of `fft` or a spectral op.",
                vec![req("chain", SIGNAL)],
                Build::Ifft,
            ),
            OpSpec::new(
                "freeze",
                "Holds the current spectral frame when `freeze` > 0: `chain.freeze(trigger)`. While frozen, magnitudes are locked but phases keep advancing to avoid clicks.",
                vec![req("chain", SIGNAL), opt("freeze", SIGNAL)],
                Build::SpectralFreeze,
            ),
            OpSpec::new(
                "spectral_delay",
                "Per-bin spectral delay: `chain.spectral_delay(time: 0.2, spread: 0.5, feedback: 0.4)`. Higher bins are delayed more when `spread` > 0. Feedback re-injects delayed energy.",
                vec![
                    req("chain", SIGNAL),
                    opt("time", SIGNAL),
                    opt("spread", SIGNAL),
                    opt("feedback", SIGNAL),
                ],
                Build::SpectralDelay,
            ),
        ];
        Registry { ops }
    }

    pub fn find(&self, name: &str) -> Option<&OpSpec> {
        self.ops.iter().find(|o| o.name == name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.ops.iter().map(|o| o.name.as_str())
    }
}

pub const WAVES: &[(&str, Wave)] = &[
    ("sine", Wave::Sine),
    ("saw", Wave::Saw),
    ("square", Wave::Square),
    ("tri", Wave::Tri),
];

pub fn wave(name: &str) -> Option<Wave> {
    WAVES.iter().find(|(n, _)| *n == name).map(|(_, w)| *w)
}

/// The one-argument math functions.
pub fn math_fn(name: &str) -> Option<Fn1> {
    Fn1::ALL.into_iter().find(|f| f.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_cycle_is_analysed_once_while_it_is_in_use() {
        let cycle = [0.0, 0.5, 1.0, 0.5, 0.0, -0.5, -1.0, -0.5, 0.123];
        let (WaveArg::Table(a), WaveArg::Table(b)) = (
            WaveArg::table(&cycle).unwrap(),
            WaveArg::table(&cycle).unwrap(),
        ) else {
            panic!("tables")
        };
        assert!(Arc::ptr_eq(&a, &b));
        let WaveArg::Table(other) = WaveArg::table(&cycle[..8]).unwrap() else {
            panic!("table")
        };
        assert!(!Arc::ptr_eq(&a, &other));
    }
}
