use crate::opcode::{Opcode, Port, ProcessCtx};

// The classic Freeverb network: eight parallel lowpass-feedback combs into four
// series allpasses. Lengths are for 44.1kHz and scaled to the sample rate.
const COMB_LENGTHS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_LENGTHS: [usize; 4] = [556, 441, 341, 225];
const INPUT_GAIN: f32 = 0.015;
const WET_SCALE: f32 = 3.0;
const ALLPASS_FEEDBACK: f32 = 0.5;

fn flush(x: f32) -> f32 {
    if x.abs() < 1e-20 { 0.0 } else { x }
}

#[derive(Clone)]
struct Comb {
    buf: Vec<f32>,
    pos: usize,
    store: f32,
}

#[derive(Clone)]
struct Allpass {
    buf: Vec<f32>,
    pos: usize,
}

/// Mono Freeverb. `room` (0..1) sets the decay time and `damp` (0..1) how fast
/// highs die away. Output is the reverberated signal only.
#[derive(Clone)]
pub struct Reverb {
    combs: Vec<Comb>,
    allpasses: Vec<Allpass>,
}

impl Reverb {
    pub fn new() -> Self {
        let mut r = Reverb { combs: Vec::new(), allpasses: Vec::new() };
        r.prepare(48_000.0);
        r
    }
}

impl Default for Reverb {
    fn default() -> Self {
        Self::new()
    }
}

impl Opcode for Reverb {
    fn name(&self) -> &str {
        "reverb"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x"), Port::optional("room", 0.5), Port::optional("damp", 0.5)];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Reverb::new())
    }

    fn prepare(&mut self, sample_rate: f32) {
        let scale = |n: usize| ((n as f32 * sample_rate / 44_100.0).round() as usize).max(1);
        self.combs = COMB_LENGTHS.iter().map(|&n| Comb { buf: vec![0.0; scale(n)], pos: 0, store: 0.0 }).collect();
        self.allpasses = ALLPASS_LENGTHS.iter().map(|&n| Allpass { buf: vec![0.0; scale(n)], pos: 0 }).collect();
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            let feedback = inputs[1][i].clamp(0.0, 1.0) * 0.28 + 0.7;
            let damp1 = inputs[2][i].clamp(0.0, 1.0) * 0.4;
            let damp2 = 1.0 - damp1;

            let input = inputs[0][i] * INPUT_GAIN;
            let mut sum = 0.0;
            for c in &mut self.combs {
                let y = c.buf[c.pos];
                c.store = flush(y * damp2 + c.store * damp1);
                c.buf[c.pos] = input + c.store * feedback;
                c.pos = (c.pos + 1) % c.buf.len();
                sum += y;
            }
            for a in &mut self.allpasses {
                let buffered = a.buf[a.pos];
                let y = buffered - sum;
                a.buf[a.pos] = flush(sum + buffered * ALLPASS_FEEDBACK);
                a.pos = (a.pos + 1) % a.buf.len();
                sum = y;
            }
            *o = sum * WET_SCALE;
        }
    }
}
