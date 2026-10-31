//! The language's view of built-in opcodes: names, argument order, and the unit
//! each argument expects. Argument names match the engine's port names. User
//! functions are compiled where they are called (see `lower` and `kernel`).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};

use niminal_engine::Opcode;
pub use niminal_engine::ops::Wave;
use niminal_engine::ops::{
    Allpass, Biquad, BiquadMode, Bitcrush, Chorus, Clip, Comb, Compressor, Convolve, DcBlock,
    Delay, Distortion, Eq, EqMode, Fdn, FilterMode, FinishWhen, Fn1, Fold, Follow, Formant, Freeze,
    Func, GLITCH_MAX_SECONDS, Gain, Gate, Granular, Impulse, IrSpectrum, Ladder, Lag, Limiter,
    Modal, Noise, NoiseColor, Osc, PartialMorph, Phaser, Phasor, PitchShift, RepeatLast, Resonator,
    Reverb, RewindLast, SampleHold, Scatter, SlowDown, SpectralDelay, SpectralEdit,
    SpectralEditKind, SpectralFreeze, SpectralMemory, SpectralMorph, SpectralPartials,
    SpectralWarp, Svf, Waveshape, Wavetable,
};

use crate::unit::Unit;

mod granular;

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
    /// Numbers known while compiling, such as an impulse response: a table, or one per channel.
    Table,
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

    /// One table is a single cycle; several are a bank to morph between.
    pub fn from_tables(tables: &[Arc<niminal_engine::Table>]) -> Result<Self, String> {
        match tables {
            [one] => Self::from_table(one),
            many => Self::bank(many),
        }
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
    /// The table for a `Kind::Table` parameter: the one for this copy's channel.
    pub table: Option<Arc<niminal_engine::Table>>,
    /// Named table arguments, including every channel of a source.
    pub tables: HashMap<String, Vec<Arc<niminal_engine::Table>>>,
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

/// Impulse responses are transformed once while any program still uses them.
fn cached_ir(table: &niminal_engine::Table) -> Arc<IrSpectrum> {
    static MADE: LazyLock<Mutex<HashMap<Vec<u32>, Weak<IrSpectrum>>>> =
        LazyLock::new(Mutex::default);
    let key: Vec<u32> = table.data().iter().map(|v| v.to_bits()).collect();
    let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ir) = made.get(&key).and_then(Weak::upgrade) {
        return ir;
    }
    made.retain(|_, ir| ir.strong_count() > 0);
    let ir = Arc::new(IrSpectrum::from_slice(table.data()));
    made.insert(key, Arc::downgrade(&ir));
    ir
}

/// The `size:` of a spectral effect.
fn fft_size(a: &BuildArgs) -> Result<usize, String> {
    let size = a.consts.get("size").map_or(1024, |s| s.round() as usize);
    if niminal_engine::ops::valid_fft_size(size) {
        Ok(size)
    } else {
        Err(format!(
            "`size` must be a power of two from {} to 8192, not {size}",
            niminal_engine::block_size() * 2
        ))
    }
}

/// Settings shared by the peak vocoder and its fixed-capacity spectral memory.
fn spectral_edit(a: &BuildArgs, kind: SpectralEditKind) -> Result<Box<dyn Opcode>, String> {
    let (size, overlap, _) = vocoder_settings(a)?;
    Ok(Box::new(SpectralEdit::new(kind, size, overlap)))
}

fn partial_capacity(a: &BuildArgs) -> Result<usize, String> {
    let count = a.consts.get("max_tracks").copied().unwrap_or(64.0);
    if count.fract() != 0.0 || !(1.0..=256.0).contains(&count) {
        return Err("max_tracks must be a whole number from 1 to 256".into());
    }
    Ok(count as usize)
}

fn vocoder_settings(a: &BuildArgs) -> Result<(usize, usize, usize), String> {
    let size = a.consts.get("size").copied().unwrap_or(2048.0);
    if size.fract() != 0.0 || !(4.0..=65536.0).contains(&size) || !(size as usize).is_power_of_two()
    {
        return Err("spectral size must be a power of two from 4 to 65536".into());
    }
    let overlap = a.consts.get("overlap").copied().unwrap_or(4.0);
    if ![2.0, 4.0, 8.0, 16.0].contains(&overlap) || overlap > size {
        return Err("spectral overlap must be 2, 4, 8 or 16 and no larger than size".into());
    }
    let detail = a.consts.get("detail").copied().unwrap_or(32.0);
    if detail.fract() != 0.0 || !(1.0..=65536.0).contains(&detail) {
        return Err("spectral detail must be a whole number from 1 to 65536".into());
    }
    Ok((size as usize, overlap as usize, detail as usize))
}

/// The longest delay line a program may ask for.
const MAX_DELAY_SECONDS: f64 = 30.0;

/// The `max:` of a glitch opcode: how much sound it keeps, in seconds.
fn glitch_max(a: &BuildArgs) -> Result<f32, String> {
    let max = a
        .consts
        .get("max")
        .copied()
        .unwrap_or(f64::from(GLITCH_MAX_SECONDS));
    if max > MAX_DELAY_SECONDS {
        return Err(format!("`max` can be at most {MAX_DELAY_SECONDS} seconds"));
    }
    Ok(max.max(0.0) as f32)
}

pub enum Build {
    Builtin(BuildFn),
    /// `pan`: one output per speaker of the master layout.
    Pan,
    /// `to_layout`: converts between layouts.
    ToLayout,
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

/// A `biquad_` filter: the signal, a cutoff and a `q`.
fn biquad(name: &str, doc: &str, build: BuildFn) -> OpSpec {
    OpSpec::new(
        name,
        doc,
        vec![
            req("x", SIGNAL),
            req("cutoff", Kind::Signal(Unit::Hz)),
            opt("q", SIGNAL),
        ],
        Build::Builtin(build),
    )
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
                "An oscillator: `osc(saw, 440hz)`. The waveform is sine, saw, square, pulse or tri, a table (`harmonics([...])`), or a `bank([...])` of tables; `phase_mod` bends the phase for FM; `position` morphs a bank; `width` controls pulse duty cycle (0.01..0.99); `phase` sets the starting/reset phase in cycles; `sync` resets at a master frequency in hz (0 disables sync). Hard sync uses fractional reset timing and polynomial discontinuity corrections.",
                vec![
                    req("wave", Kind::Wave),
                    req("freq", Kind::Signal(Unit::Hz)),
                    opt("phase_mod", SIGNAL),
                    opt("position", SIGNAL),
                    opt("width", SIGNAL),
                    opt("phase", SIGNAL),
                    opt("sync", Kind::Signal(Unit::Hz)),
                ],
                Build::Builtin(|a| {
                    Ok(Box::new(match a.wave.clone().expect("osc has a wave") {
                        WaveArg::Builtin(wave) => Osc::new(wave),
                        WaveArg::Table(table) => Osc::with_table(table),
                        WaveArg::Bank(bank) => Osc::with_bank(bank),
                    }))
                }),
            ),
            OpSpec::new(
                "phasor",
                "A normalized phase ramp (0..1) for table readers and custom oscillators. `phase` sets its starting/reset phase in cycles; `sync` is a master frequency in hz (0 disables hard sync). Negative frequency runs backwards. The ramp itself is not band-limited audio.",
                vec![
                    req("freq", Kind::Signal(Unit::Hz)),
                    opt("phase", SIGNAL),
                    opt("sync", Kind::Signal(Unit::Hz)),
                ],
                Build::Builtin(|_| Ok(Box::new(Phasor::default()))),
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
                "A delay line. `time` is how long to wait, `feedback` feeds the output back in, and `damp` (a cutoff in hz, off by default) darkens each echo as it goes round, like a tape delay. For several echoes, add several delays: `x + x.delay(120ms) * 0.6 + x.delay(250ms) * 0.3`, each with its own time and processing. A changing `time` needs `max:` to size the line (at most 30 seconds).",
                vec![
                    req("x", SIGNAL),
                    req("time", Kind::Signal(Unit::Time)),
                    opt("feedback", SIGNAL),
                    opt("damp", Kind::Signal(Unit::Hz)),
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
                "fdn_reverb",
                "A reverb from a feedback delay network of eight lines. `time` is how long the tail takes to fall by 60db, `damp` the cutoff of the low-pass in each line (highs die first, as in a room), and `size` stretches the lines (0.5 to 2). Wet only; the channels of a stereo signal are decorrelated.",
                vec![
                    req("x", SIGNAL),
                    opt("time", Kind::Signal(Unit::Time)),
                    opt("damp", Kind::Signal(Unit::Hz)),
                    opt("size", SIGNAL),
                ],
                Build::Builtin(|a| Ok(Box::new(Fdn::for_channel(a.channel)))),
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
                "A compressor. `threshold` and `makeup` take db or a plain factor; `ratio`, `attack` and `release` shape the envelope. `knee` rounds the corner at the threshold over that many db (`knee: 6db`; the default is a hard knee). Pass `sidechain:` to key off another signal (for ducking, or one shared level for both sides of a stereo sound).",
                vec![
                    req("x", SIGNAL),
                    opt("threshold", Kind::Gain),
                    opt("ratio", SIGNAL),
                    opt("attack", Kind::Signal(Unit::Time)),
                    opt("release", Kind::Signal(Unit::Time)),
                    opt("makeup", Kind::Gain),
                    opt("sidechain", SIGNAL),
                    opt("knee", Kind::Gain),
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
                "Reduces sample rate and bit depth. `rate` is how often a new sample is taken (hz); `bits` is the bit depth (from 2; silence stays silent).",
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
                "repeat_last",
                "Repeats the last `length` of the sound for as long as `hold` is above zero, then lets the sound through again. Each repeat is `fade` times the level of the one before and `shrink` times the length, so a `shrink` below 1 rolls faster the longer it is held. `pitch` (in semitones) changes the speed the piece is read at. `length` is read once at the start of each repeat, so a length that moves (a ramp from 1/4 beat to 1/32) gives a run of clean repeats. `gate` (0 to 1) is how much of each repeat sounds, the rest being silence, and a `reverse` above zero plays each repeat backwards. `shape` gives each repeat a level curve: above zero it dies away (1 is a straight fade to nothing), below zero it swells in. `damp` (0 to 1) darkens each repeat more than the one before, like a tape echo. `attack` (0 to 1) is how much of the repeat it takes to rise to full level, `bend` (0 to 1) turns the straight `shape` curve into an exponential one for a pluck, and `accent` (0 to 1) makes every second repeat that much quieter. The change fades in and out over `blend` (3ms). `max:` is the longest `length` (default 2 seconds, at most 30).",
                vec![
                    req("x", SIGNAL),
                    req("hold", SIGNAL),
                    req("length", Kind::Signal(Unit::Time)),
                    opt("pitch", SIGNAL),
                    opt("fade", SIGNAL),
                    opt("shrink", SIGNAL),
                    opt("blend", Kind::Signal(Unit::Time)),
                    opt("gate", SIGNAL),
                    opt("reverse", SIGNAL),
                    opt("shape", SIGNAL),
                    opt("damp", SIGNAL),
                    opt("attack", SIGNAL),
                    opt("bend", SIGNAL),
                    opt("accent", SIGNAL),
                    setting("max", Kind::Signal(Unit::Time)),
                ],
                Build::Builtin(|a| Ok(Box::new(RepeatLast::new(glitch_max(a)?)))),
            ),
            OpSpec::new(
                "rewind_last",
                "Plays the last `length` of the sound backwards, over and over, for as long as `hold` is above zero. `blend` is the fade in and out (3ms). `max:` is the longest `length` (default 2 seconds, at most 30).",
                vec![
                    req("x", SIGNAL),
                    req("hold", SIGNAL),
                    req("length", Kind::Signal(Unit::Time)),
                    opt("blend", Kind::Signal(Unit::Time)),
                    setting("max", Kind::Signal(Unit::Time)),
                ],
                Build::Builtin(|a| Ok(Box::new(RewindLast::new(glitch_max(a)?)))),
            ),
            OpSpec::new(
                "slow_down",
                "Slows the sound to a stop over `time` while `hold` is above zero, like a tape machine switched off, and plays it live again when `hold` ends. `blend` is the fade in and out (3ms). `max:` is how much sound is kept (default 2 seconds, at most 30).",
                vec![
                    req("x", SIGNAL),
                    req("hold", SIGNAL),
                    req("time", Kind::Signal(Unit::Time)),
                    opt("blend", Kind::Signal(Unit::Time)),
                    setting("max", Kind::Signal(Unit::Time)),
                ],
                Build::Builtin(|a| Ok(Box::new(SlowDown::new(glitch_max(a)?)))),
            ),
            OpSpec::new(
                "scatter",
                "Cuts the last `length` of the sound (a bar, say) into `slices` and plays them back in a shuffled order for as long as `hold` is above zero, then lets the sound through again. Each pass through the pieces is a new shuffle, and the same `seed` gives the same shuffles every time, so a seed you like is a glitch you can keep. Each slice then rolls the dice, all from the same seed: `reverse` (0 to 1) is the chance it plays backwards, `repeat` the chance it is swapped for a slice picked at random (so some come twice and some not at all), `drop` the chance it is silent, and `detune` is how many semitones, up to, it may be shifted either way. `slices` is 2 to 32 (default 8). `blend` is the fade in and out (3ms). `max:` is the longest `length` (default 2 seconds, at most 30).",
                vec![
                    req("x", SIGNAL),
                    req("hold", SIGNAL),
                    req("length", Kind::Signal(Unit::Time)),
                    opt("slices", SIGNAL),
                    opt("seed", SIGNAL),
                    opt("reverse", SIGNAL),
                    opt("repeat", SIGNAL),
                    opt("drop", SIGNAL),
                    opt("detune", SIGNAL),
                    opt("blend", Kind::Signal(Unit::Time)),
                    setting("max", Kind::Signal(Unit::Time)),
                ],
                Build::Builtin(|a| Ok(Box::new(Scatter::new(glitch_max(a)?)))),
            ),
            OpSpec::new(
                "freeze",
                "Holds the last `length` of the sound (default 250ms) as a cloud of tiny grains for as long as `hold` is above zero, so a moment of the sound hangs in the air while the rest carries on unheard. A grain of `grain` seconds (80ms) starts `density` times per grain length (4), each at a place picked around `position` (0 is the start of the held piece, 1 the end) by up to `spread` (0 to 1): a spread of 0 holds one spot and a wide spread smears the whole piece. `pitch` (semitones) shifts every grain and `detune` adds up to that many semitones either way at random. The same `seed` gives the same cloud every time. `blend` is the fade in and out (10ms). `max:` is the most sound kept (default 2 seconds, at most 30).",
                vec![
                    req("x", SIGNAL),
                    req("hold", SIGNAL),
                    opt("length", Kind::Signal(Unit::Time)),
                    opt("grain", Kind::Signal(Unit::Time)),
                    opt("density", SIGNAL),
                    opt("position", SIGNAL),
                    opt("spread", SIGNAL),
                    opt("pitch", SIGNAL),
                    opt("detune", SIGNAL),
                    opt("seed", SIGNAL),
                    opt("blend", Kind::Signal(Unit::Time)),
                    setting("max", Kind::Signal(Unit::Time)),
                ],
                Build::Builtin(|a| Ok(Box::new(Freeze::new(glitch_max(a)?)))),
            ),
            OpSpec::new(
                "ladder",
                "A four-pole low-pass with a `tanh` in each stage, like a transistor ladder. `res` feeds the output back to the input: 0 is a plain low-pass, and near 1 it rings and then sings at the cutoff. `drive` scales the input, so loud sounds saturate the stages harder.",
                vec![
                    req("x", SIGNAL),
                    req("cutoff", Kind::Signal(Unit::Hz)),
                    opt("res", SIGNAL),
                    opt("drive", SIGNAL),
                ],
                Build::Builtin(|_| Ok(Box::new(Ladder::default()))),
            ),
            OpSpec::new(
                "biquad",
                "One two-pole, two-zero filter section from its five coefficients (`a0` already divided out), for building filters of your own: `x.biquad(b0, b1, b2, a1, a2)`. For the usual shapes use `biquad_lp`, `biquad_hp`, `biquad_bp`, `biquad_notch` or `biquad_allpass`.",
                vec![
                    req("x", SIGNAL),
                    req("b0", SIGNAL),
                    req("b1", SIGNAL),
                    req("b2", SIGNAL),
                    req("a1", SIGNAL),
                    req("a2", SIGNAL),
                ],
                Build::Builtin(|_| Ok(Box::new(Biquad::new(BiquadMode::Raw)))),
            ),
            biquad(
                "biquad_lp",
                "A two-pole low-pass. `q` is the height of the peak at the cutoff: 0.707 is flat.",
                |_| Ok(Box::new(Biquad::new(BiquadMode::Low))),
            ),
            biquad("biquad_hp", "A two-pole high-pass.", |_| {
                Ok(Box::new(Biquad::new(BiquadMode::High)))
            }),
            biquad(
                "biquad_bp",
                "A two-pole band-pass with a constant peak gain of 1; `q` sets how narrow it is.",
                |_| Ok(Box::new(Biquad::new(BiquadMode::Band))),
            ),
            biquad(
                "biquad_notch",
                "A notch: everything but a narrow band around `cutoff`.",
                |_| Ok(Box::new(Biquad::new(BiquadMode::Notch))),
            ),
            biquad(
                "biquad_allpass",
                "Passes every frequency at the same level and turns the phase around `cutoff`; `q` sets how sharply.",
                |_| Ok(Box::new(Biquad::new(BiquadMode::All))),
            ),
            OpSpec::new(
                "modal",
                "Resonators in parallel, one for each of `ratios` times `freq`: modal synthesis of bars, bells and drums. `x.modal(freq, [1, 2.756, 5.404], decay: 500ms)`. `decay` is how long every mode takes to fall by 60db (300ms unless given). Each mode is about as loud as the input at its own frequency. Modes above half the sample rate are left out.",
                vec![
                    req("x", SIGNAL),
                    req("freq", Kind::Signal(Unit::Hz)),
                    req("ratios", Kind::Table),
                    opt("decay", Kind::Signal(Unit::Time)),
                ],
                Build::Builtin(|a| {
                    let ratios = a.table.clone().expect("modal has ratios");
                    Ok(Box::new(Modal::new(ratios)))
                }),
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
                "waveshape",
                "Maps a signal in -1..1 through a transfer table: `osc(saw, freq).waveshape(cheby([1, 0.3]))`. Values outside -1..1 are clamped to the ends.",
                vec![req("x", SIGNAL), req("table", Kind::Table)],
                Build::Builtin(|a| {
                    let table = a.table.clone().expect("waveshape has a table");
                    Ok(Box::new(Waveshape::new(table)))
                }),
            ),
            granular::spec(),
            OpSpec::new(
                "convolve",
                "Convolution with an impulse response: `x.convolve(hall.data, mix: 0.3)` for a `sample hall = \"hall.wav\"`. A stereo response convolves each channel of `x` with its own. Partitioned for real-time use; `mix` blends wet with dry. No latency.",
                vec![req("x", SIGNAL), req("ir", Kind::Table), opt("mix", SIGNAL)],
                Build::Builtin(|a| {
                    let ir = a.table.as_ref().expect("convolve has an impulse response");
                    Ok(Box::new(Convolve::new(cached_ir(ir))))
                }),
            ),
            OpSpec::new(
                "spectral_warp",
                "Peak-region spectral pitch and formant shifting. `shift` and `formant` are semitones; `preserve` holds the original spectral envelope as pitch changes. `transients` reinitializes growing peak regions on attacks. Size defaults to 2048, overlap to 4, detail to 32. Latency is size samples.",
                vec![
                    req("x", SIGNAL),
                    opt("shift", SIGNAL),
                    opt("formant", SIGNAL),
                    opt("preserve", SIGNAL),
                    opt("transients", SIGNAL),
                    setting("size", SIGNAL),
                    setting("overlap", SIGNAL),
                    setting("detail", SIGNAL),
                ],
                Build::Builtin(|a| {
                    let (size, overlap, detail) = vocoder_settings(a)?;
                    Ok(Box::new(SpectralWarp::new(size, overlap, detail)))
                }),
            ),
            OpSpec::new(
                "spectral_morph",
                "Interpolate two aligned spectra, with independent magnitude and instantaneous-frequency blends in 0..1. Endpoints reproduce their sources; intermediate values are bin-based morphs, not tracked-partial matching. Size defaults to 2048, overlap to 4. Latency is size samples.",
                vec![
                    req("x", SIGNAL),
                    req("other", SIGNAL),
                    opt("magnitude", SIGNAL),
                    opt("frequency", SIGNAL),
                    setting("size", SIGNAL),
                    setting("overlap", SIGNAL),
                ],
                Build::Builtin(|a| {
                    let (size, overlap, _) = vocoder_settings(a)?;
                    Ok(Box::new(SpectralMorph::new(size, overlap)))
                }),
            ),
            OpSpec::new(
                "partial_morph",
                "Track and pair partials from two inputs by relative frequency, then morph magnitudes linearly and frequencies logarithmically. Unmatched tracks fade independently. Count (32) and threshold (0.01) apply per source; freeze holds both track banks while blends remain live. Lowest detected tracks anchor relative matching, not a pitch detector. Size defaults to 2048, overlap to 4, max_tracks to 64 (1..256 per source). Tonal resynthesis only; latency size/2 + hop samples.",
                vec![
                    req("x", SIGNAL),
                    req("other", SIGNAL),
                    opt("magnitude", SIGNAL),
                    opt("frequency", SIGNAL),
                    opt("count", SIGNAL),
                    opt("threshold", SIGNAL),
                    opt("freeze", SIGNAL),
                    setting("size", SIGNAL),
                    setting("overlap", SIGNAL),
                    setting("max_tracks", SIGNAL),
                ],
                Build::Builtin(|a| {
                    let (size, overlap, _) = vocoder_settings(a)?;
                    Ok(Box::new(PartialMorph::new(
                        size,
                        overlap,
                        partial_capacity(a)?,
                    )))
                }),
            ),
            OpSpec::new(
                "spectral_partials",
                "Track spectral peaks in stable slots and resynthesize with interpolated sine oscillators. Count keeps the strongest candidates (default 32); threshold is relative to the frame maximum (0.01). Shift is semitones, freeze holds tracks, track selects a 1-based slot (0 all), lowest/highest are Hz. Max_tracks is compile-time 1..256 (default 64). Slots can be reused after tracks die. Latency is size/2 + hop samples.",
                vec![
                    req("x", SIGNAL),
                    opt("count", SIGNAL),
                    opt("threshold", SIGNAL),
                    opt("shift", SIGNAL),
                    opt("freeze", SIGNAL),
                    opt("track", SIGNAL),
                    opt("lowest", Kind::Signal(Unit::Hz)),
                    opt("highest", Kind::Signal(Unit::Hz)),
                    setting("max_tracks", SIGNAL),
                    setting("size", SIGNAL),
                    setting("overlap", SIGNAL),
                ],
                Build::Builtin(|a| {
                    let (size, overlap, _) = vocoder_settings(a)?;
                    Ok(Box::new(SpectralPartials::new(
                        size,
                        overlap,
                        partial_capacity(a)?,
                    )))
                }),
            ),
            OpSpec::new(
                "spectral_memory",
                "Capture a ring of spectral frames with record above 0.5; below it, scan the capture with position (0 oldest, 1 newest), independently of pitch. Shift, formant, preserve and transients are as for spectral_warp; transient reset defaults off. Frames defaults to 256. Storage is fixed at compile time, at most 64 MiB per instance. Latency is size samples.",
                vec![
                    req("x", SIGNAL),
                    opt("record", SIGNAL),
                    opt("position", SIGNAL),
                    opt("shift", SIGNAL),
                    opt("formant", SIGNAL),
                    opt("preserve", SIGNAL),
                    opt("transients", SIGNAL),
                    setting("frames", SIGNAL),
                    setting("size", SIGNAL),
                    setting("overlap", SIGNAL),
                    setting("detail", SIGNAL),
                ],
                Build::Builtin(|a| {
                    let (size, overlap, detail) = vocoder_settings(a)?;
                    let frames = a.consts.get("frames").copied().unwrap_or(256.0);
                    if frames.fract() != 0.0 || !(1.0..=65536.0).contains(&frames) {
                        return Err(
                            "spectral memory frames must be a whole number from 1 to 65536".into(),
                        );
                    }
                    let frames = frames as usize;
                    if frames
                        .saturating_mul(size / 2 + 1)
                        .saturating_mul(3 * std::mem::size_of::<f32>())
                        > 64 * 1024 * 1024
                    {
                        return Err("spectral memory exceeds 64 MiB; reduce frames or size".into());
                    }
                    Ok(Box::new(SpectralMemory::new(size, overlap, frames, detail)))
                }),
            ),
            OpSpec::new(
                "spectral_freeze",
                "Holds the spectrum it hears while `freeze` is above 0.5: `pad.spectral_freeze(freeze: gate)`. Magnitudes are locked and phases keep turning, so the held sound is steady. `size:` is the FFT size, a power of two from twice the block size to 8192 (default 1024). Latency is `size` samples.",
                vec![
                    req("x", SIGNAL),
                    opt("freeze", SIGNAL),
                    setting("size", SIGNAL),
                ],
                Build::Builtin(|a| Ok(Box::new(SpectralFreeze::new(fft_size(a)?)))),
            ),
            OpSpec::new(
                "spectral_delay",
                "Delays each frequency bin separately: `x.spectral_delay(time: 300ms, spread: 0.5, feedback: 0.4)`. With `spread` above 0 the lower bins are delayed less than the higher ones. A changing `time` needs `max:` (default 2 seconds, at most 30). `size:` is the FFT size as for `spectral_freeze`.",
                vec![
                    req("x", SIGNAL),
                    opt("time", Kind::Signal(Unit::Time)),
                    opt("spread", SIGNAL),
                    opt("feedback", SIGNAL),
                    setting("max", Kind::Signal(Unit::Time)),
                    setting("size", SIGNAL),
                ],
                Build::Builtin(|a| {
                    let max = a
                        .consts
                        .get("max")
                        .or_else(|| a.consts.get("time"))
                        .copied()
                        .unwrap_or(2.0);
                    if max > MAX_DELAY_SECONDS {
                        return Err(format!(
                            "a spectral delay can be at most {MAX_DELAY_SECONDS} seconds long"
                        ));
                    }
                    Ok(Box::new(SpectralDelay::new(
                        fft_size(a)?,
                        max.max(0.0) as f32,
                    )))
                }),
            ),
        ];
        let mut ops = ops;
        for (name, doc, kind, controls) in [
            (
                "spectral_thin",
                "Keep the count loudest FFT bins (default 16). Count 0 is silence; size/2+1 keeps all. Bin selection is frame-local, not partial tracking.",
                SpectralEditKind::Thin,
                vec![opt("count", SIGNAL)],
            ),
            (
                "spectral_arp",
                "Focus the spectrum around position (0..1 of Nyquist), with depth (0..1), gain (linear) and width (FFT bins, at least 1). Animate position to arpeggiate spectral components.",
                SpectralEditKind::Arp,
                vec![
                    opt("position", SIGNAL),
                    opt("depth", SIGNAL),
                    opt("gain", SIGNAL),
                    opt("width", SIGNAL),
                ],
            ),
            (
                "spectral_hold",
                "Freeze magnitude and frequency independently when their controls exceed 0.5. Unheld components keep following the input. Frequencies use measured instantaneous phase advance.",
                SpectralEditKind::Hold,
                vec![opt("magnitude", SIGNAL), opt("frequency", SIGNAL)],
            ),
            (
                "spectral_shift",
                "Add shift Hz to peak regions at or above lowest Hz. Unlike semitone shifting, this makes harmonic sounds inharmonic. Out-of-band bins are discarded.",
                SpectralEditKind::Shift,
                vec![
                    opt("shift", Kind::Signal(Unit::Hz)),
                    opt("lowest", Kind::Signal(Unit::Hz)),
                ],
            ),
        ] {
            let mut params = vec![req("x", SIGNAL)];
            params.extend(controls);
            params.extend([setting("size", SIGNAL), setting("overlap", SIGNAL)]);
            ops.push(OpSpec::new(
                name,
                doc,
                params,
                Build::Builtin(match kind {
                    SpectralEditKind::Thin => |a| spectral_edit(a, SpectralEditKind::Thin),
                    SpectralEditKind::Arp => |a| spectral_edit(a, SpectralEditKind::Arp),
                    SpectralEditKind::Hold => |a| spectral_edit(a, SpectralEditKind::Hold),
                    SpectralEditKind::Shift => |a| spectral_edit(a, SpectralEditKind::Shift),
                }),
            ));
        }
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
    ("pulse", Wave::Pulse),
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
            WaveArg::from_table(&niminal_engine::Table::from_f64_slice(&cycle, None)).unwrap(),
            WaveArg::from_table(&niminal_engine::Table::from_f64_slice(&cycle, None)).unwrap(),
        ) else {
            panic!("tables")
        };
        assert!(Arc::ptr_eq(&a, &b));
        let WaveArg::Table(other) =
            WaveArg::from_table(&niminal_engine::Table::from_f64_slice(&cycle[..8], None)).unwrap()
        else {
            panic!("table")
        };
        assert!(!Arc::ptr_eq(&a, &other));
    }
}
