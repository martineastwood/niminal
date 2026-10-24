//! A feedback delay network reverb: eight delay lines mixed by a Hadamard matrix.

use crate::denormals::flush32 as flush;
use crate::opcode::{Opcode, Port, ProcessCtx, TAIL_LEVEL};

/// Line lengths in samples at 48kHz, mutually prime so the lines don't ring in step.
const LENGTHS: [f32; 8] = [1031.0, 1327.0, 1523.0, 1871.0, 2053.0, 2381.0, 2713.0, 3079.0];
/// The longest `size` a line has room for.
const MAX_SIZE: f32 = 2.0;
/// Samples (at 48kHz) added to every line for each successive channel, so the channels of a
/// multichannel reverb are decorrelated.
const CHANNEL_SPREAD: f32 = 23.0;
/// What each line takes of the input, and what the output takes of each line.
const GAIN: f32 = 0.35;
/// Signs the lines have in the output.
const SIGNS: [f32; 8] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
/// How much the output can be louder than the loudest thing stored in the lines.
const OUTPUT_BOUND: f32 = 8.0 * GAIN;
/// The 8 x 8 Hadamard matrix is scaled by the square root of 8, so it mixes without gaining or losing.
const HADAMARD_SCALE: f32 = 0.353_553_4;

/// A feedback delay network reverb. `time` is how long the tail takes to fall by 60db, `damp` the
/// cutoff in hz of the low-pass in each line (highs die first, as in a room), and `size` stretches
/// all the lines (0.5 to 2). It is wet only. The three are read once a block.
#[derive(Clone)]
pub struct Fdn {
    lines: Vec<Vec<f32>>,
    /// Where each line writes next; the lines are different lengths, so each goes round on its own.
    pos: [usize; 8],
    /// The low-pass memory of each line.
    lowpass: [f32; 8],
    /// Consecutive samples for which nothing stored in the lines was audible.
    quiet_run: usize,
    /// How long the longest line takes to go round once.
    longest: usize,
    channel: usize,
    sample_rate: f32,
}

impl Fdn {
    pub fn for_channel(channel: usize) -> Self {
        let mut fdn = Fdn {
            lines: Vec::new(),
            pos: [0; 8],
            lowpass: [0.0; 8],
            quiet_run: 0,
            longest: 0,
            channel,
            sample_rate: 48_000.0,
        };
        fdn.prepare(48_000.0);
        fdn
    }

    /// Line `i` at `size 1`, in samples at the current sample rate.
    fn base_length(&self, i: usize) -> f32 {
        (LENGTHS[i] + self.channel as f32 * CHANNEL_SPREAD) * self.sample_rate / 48_000.0
    }
}

impl Default for Fdn {
    fn default() -> Self {
        Self::for_channel(0)
    }
}

impl Opcode for Fdn {
    fn reset(&mut self) {
        for line in &mut self.lines {
            line.fill(0.0);
        }
        self.pos = [0; 8];
        self.lowpass = [0.0; 8];
        self.quiet_run = self.longest;
    }

    fn name(&self) -> &str {
        "fdn_reverb"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("time", 2.0),
            Port::optional("damp", 5000.0),
            Port::optional("size", 1.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        let mut fresh = Fdn::for_channel(self.channel);
        fresh.prepare(self.sample_rate);
        Box::new(fresh)
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        // room for the longest line at the largest size, and a sample either side for interpolating
        self.lines = (0..8).map(|i| vec![0.0; (self.base_length(i) * MAX_SIZE).ceil() as usize + 4]).collect();
        self.longest = self.lines.iter().map(Vec::len).max().unwrap_or(0);
        self.pos = [0; 8];
        self.lowpass = [0.0; 8];
        self.quiet_run = self.longest;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>()
            && self.channel == old.channel
            && self.longest == old.longest
        {
            std::mem::swap(&mut self.lines, &mut old.lines);
            self.pos = old.pos;
            self.lowpass = old.lowpass;
            self.quiet_run = old.quiet_run;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        if out.is_empty() {
            return;
        }
        let sr = self.sample_rate.max(1.0);
        let time = inputs[1][0].max(0.01);
        let damp = inputs[2][0].clamp(20.0, sr * 0.45);
        let size = inputs[3][0].clamp(0.25, MAX_SIZE);
        let pole = (-std::f32::consts::TAU * damp / sr).exp();
        // each line loses 60db over `time`, in proportion to its length
        let mut delay = [0.0f32; 8];
        let mut feedback = [0.0f32; 8];
        for i in 0..8 {
            delay[i] = (self.base_length(i) * size).max(2.0);
            feedback[i] = (-6.9078 * delay[i] / (time * sr)).exp();
        }
        let whole: [usize; 8] = std::array::from_fn(|i| delay[i] as usize);
        let fraction: [f32; 8] = std::array::from_fn(|i| delay[i] - whole[i] as f32);
        let quiet = TAIL_LEVEL / OUTPUT_BOUND;
        for (n, o) in out.iter_mut().enumerate() {
            let x = inputs[0][n] * GAIN;
            let mut read = [0.0f32; 8];
            for i in 0..8 {
                let line = &self.lines[i];
                let len = line.len();
                // `delay` samples back, between the two stored samples either side
                let newer = (self.pos[i] + len - whole[i]) % len;
                let older = if newer == 0 { len - 1 } else { newer - 1 };
                read[i] = line[newer] + (line[older] - line[newer]) * fraction[i];
            }
            *o = (0..8).map(|i| read[i] * SIGNS[i]).sum::<f32>() * GAIN;

            // mix the lines with the 8 x 8 Hadamard matrix over the square root of 8
            let mut mixed = read;
            for stride in [1usize, 2, 4] {
                for block in (0..8).step_by(stride * 2) {
                    for j in block..block + stride {
                        let (a, b) = (mixed[j], mixed[j + stride]);
                        mixed[j] = a + b;
                        mixed[j + stride] = a - b;
                    }
                }
            }
            let mut loudest = x.abs();
            for i in 0..8 {
                self.lowpass[i] = flush(self.lowpass[i] * pole + mixed[i] * HADAMARD_SCALE * (1.0 - pole));
                let stored = flush(x + self.lowpass[i] * feedback[i]);
                loudest = loudest.max(stored.abs());
                self.lines[i][self.pos[i]] = stored;
                self.pos[i] = (self.pos[i] + 1) % self.lines[i].len();
            }
            self.quiet_run = if loudest < quiet { self.quiet_run + 1 } else { 0 };
        }
    }

    /// The tail keeps the voice alive until it has faded.
    fn is_active(&self) -> bool {
        self.quiet_run < self.longest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(fdn: &mut Fdn, input: &[f32], time: f32, damp: f32, size: f32) -> Vec<f32> {
        let ctx = ProcessCtx::new(48_000.0, true);
        let (times, damps, sizes) = (vec![time; input.len()], vec![damp; input.len()], vec![size; input.len()]);
        let mut out = vec![0.0; input.len()];
        fdn.process(&ctx, &[input, &times, &damps, &sizes], &mut out);
        out
    }

    fn impulse(len: usize) -> Vec<f32> {
        let mut x = vec![0.0; len];
        x[0] = 1.0;
        x
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn the_tail_falls_by_sixty_decibels_over_the_time() {
        let mut fdn = Fdn::default();
        let out = run(&mut fdn, &impulse(48_000 * 3), 1.0, 12_000.0, 1.0);
        let window = |from: f32| rms(&out[(from * 48_000.0) as usize..(from * 48_000.0) as usize + 4800]);
        let (early, late) = (window(0.2), window(1.2));
        // a second later is 60db down (a thousandth), give or take how the energy happens to fall
        let drop = 20.0 * (late / early).log10();
        assert!((-80.0..-45.0).contains(&drop), "fell by {drop}db in a second");
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn a_longer_time_rings_longer() {
        let tail = |time: f32| {
            let mut fdn = Fdn::default();
            let out = run(&mut fdn, &impulse(48_000 * 3), time, 8000.0, 1.0);
            rms(&out[96_000..])
        };
        assert!(tail(4.0) > tail(1.0) * 10.0);
    }

    #[test]
    fn damping_takes_the_highs_out_of_the_tail() {
        let brightness = |damp: f32| {
            let mut fdn = Fdn::default();
            let out = run(&mut fdn, &impulse(48_000), 2.0, damp, 1.0);
            let tail = &out[12_000..];
            // the difference of neighbours is mostly high frequencies
            let rough = tail.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>();
            let level = tail.iter().map(|v| v.abs()).sum::<f32>();
            rough / level
        };
        assert!(brightness(16_000.0) > brightness(1000.0) * 1.5);
    }

    #[test]
    fn the_channels_of_a_stereo_reverb_are_not_the_same() {
        let (mut left, mut right) = (Fdn::for_channel(0), Fdn::for_channel(1));
        let a = run(&mut left, &impulse(24_000), 2.0, 5000.0, 1.0);
        let b = run(&mut right, &impulse(24_000), 2.0, 5000.0, 1.0);
        let difference: f32 = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).sum();
        assert!(difference > 0.1, "{difference}");
    }

    #[test]
    fn it_goes_quiet_so_its_voice_can_end() {
        let mut fdn = Fdn::default();
        run(&mut fdn, &impulse(64), 0.2, 5000.0, 1.0);
        assert!(fdn.is_active(), "ringing just after an impulse");
        run(&mut fdn, &vec![0.0; 48_000 * 4], 0.2, 5000.0, 1.0);
        assert!(!fdn.is_active(), "still active after the tail has gone");
    }

    #[test]
    fn extreme_settings_stay_finite_and_bounded() {
        for (time, damp, size) in [(0.0, 20.0, 0.25), (30.0, 40_000.0, 2.0), (30.0, 20.0, 0.25), (0.01, 40_000.0, 2.0)] {
            let mut fdn = Fdn::default();
            let noise: Vec<f32> = (0..48_000).map(|i| ((i * 7919) % 2001) as f32 / 1000.0 - 1.0).collect();
            let out = run(&mut fdn, &noise, time, damp, size);
            assert!(out.iter().all(|v| v.is_finite() && v.abs() < 50.0), "{time} {damp} {size}");
        }
    }
}
