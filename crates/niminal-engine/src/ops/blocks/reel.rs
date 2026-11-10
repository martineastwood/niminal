use super::*;

const GRAINS: usize = 6;

/// One playing window of a splice.
#[derive(Clone, Copy, Default)]
struct Grain {
    on: bool,
    /// Where the window starts, and how far it has moved, in samples of the splice.
    start: f64,
    off: f64,
    /// How far it has travelled, and how far it will go (its length plus its fade).
    trav: f64,
    len: f64,
    /// The splice it reads, captured when it started.
    a: f64,
    sl: f64,
    pitch: f64,
    pan: f64,
    fade: f64,
    /// Joins the same sound (a whole splice), so fades at equal gain rather than equal power.
    whole: bool,
    /// Its splice continues the one before, so the loop point cross-fades into that.
    before: bool,
    /// Fading out after a retrigger, and how far.
    releasing: bool,
    release: f64,
}

/// A reel sampler: records into a reel, divides it into splices, and plays one splice back as grains.
#[derive(Clone)]
pub struct SpliceReel {
    pub(super) sample_rate: f64,
    buf: Vec<f32>,
    edges: Vec<f64>,
    fresh_at: Vec<bool>,
    splices: usize,
    n: usize,
    end: f64,
    shift_off: f64,
    w: f64,
    rec: u8,
    oa: f64,
    ob: f64,
    rec_prev: bool,
    org_prev: f64,
    shift_prev: bool,
    mark_prev: bool,
    unmark_prev: bool,
    clear_prev: bool,
    play_prev: bool,
    clk_prev: bool,
    clk_since: f64,
    clk_period: f64,
    cursor: f64,
    shift_i: f64,
    since: f64,
    speed_now: f64,
    fresh: bool,
    drawn: f64,
    last_pos: f64,
    grains: [Grain; GRAINS],
    eos: f64,
    level: f64,
}

impl SpliceReel {
    pub fn new(size: usize, splices: usize) -> Self {
        Self {
            sample_rate: 48000.0,
            buf: vec![0.0; size.max(8)],
            edges: vec![0.0; splices.max(1) + 1],
            fresh_at: vec![false; splices.max(1) + 1],
            splices: splices.max(1),
            n: 0,
            end: 0.0,
            shift_off: 0.0,
            w: 0.0,
            rec: 0,
            oa: 0.0,
            ob: 0.0,
            rec_prev: false,
            org_prev: 0.0,
            shift_prev: false,
            mark_prev: false,
            unmark_prev: false,
            clear_prev: false,
            play_prev: false,
            clk_prev: false,
            clk_since: 0.0,
            clk_period: 0.0,
            cursor: 0.0,
            shift_i: 0.0,
            since: 0.0,
            speed_now: 1.0,
            fresh: true,
            drawn: 0.0,
            last_pos: 0.0,
            grains: [Grain::default(); GRAINS],
            eos: 0.0,
            level: 0.0,
        }
    }
    fn input_ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("record", 0.0),
            Port::optional("new_splice", 0.0),
            Port::optional("sos", 0.0),
            Port::optional("choose", 0.0),
            Port::optional("shift", 0.0),
            Port::optional("mark", 0.0),
            Port::optional("unmark", 0.0),
            Port::optional("clear", 0.0),
            Port::optional("play", 1.0),
            Port::optional("speed", 1.0),
            Port::optional("grain_size", 0.0),
            Port::optional("slide", 0.0),
            Port::optional("morph", 0.33),
            Port::optional("clock", 0.0),
        ];
        PORTS
    }
    fn fresh(&self) -> Self {
        Self::new(self.buf.len(), self.splices)
    }
    /// Back to an empty reel, in place: this runs on reset, so it must not allocate.
    pub(super) fn clear(&mut self) {
        self.buf.fill(0.0);
        self.edges.fill(0.0);
        self.fresh_at.fill(false);
        self.n = 0;
        self.end = 0.0;
        self.shift_off = 0.0;
        self.w = 0.0;
        self.rec = 0;
        self.oa = 0.0;
        self.ob = 0.0;
        self.rec_prev = false;
        self.org_prev = 0.0;
        self.shift_prev = false;
        self.mark_prev = false;
        self.unmark_prev = false;
        self.clear_prev = false;
        self.play_prev = false;
        self.clk_prev = false;
        self.clk_since = 0.0;
        self.clk_period = 0.0;
        self.cursor = 0.0;
        self.shift_i = 0.0;
        self.since = 0.0;
        self.speed_now = 1.0;
        self.fresh = true;
        self.drawn = 0.0;
        self.last_pos = 0.0;
        self.grains = [Grain::default(); GRAINS];
        self.eos = 0.0;
        self.level = 0.0;
    }
    pub(super) fn carry(&mut self, old: &mut Self) {
        if self.buf.len() == old.buf.len() && self.splices == old.splices {
            let sample_rate = self.sample_rate;
            std::mem::swap(self, old);
            self.sample_rate = sample_rate;
        }
    }

    fn sample(&self, position: f64) -> f64 {
        f64::from(self.buf[crate::wrapped_index(position, self.buf.len())])
    }
    fn edge(&self, index: usize) -> f64 {
        self.edges[index % self.edges.len()]
    }

    /// One sample. `c` is the controls in port order after `x`.
    #[allow(clippy::too_many_lines)]
    fn tick(&mut self, x: f64, c: &[f64; 14]) -> [f64; 6] {
        let [record, new_splice, sos, choose, shift, mark, unmark, clear, play, speed, grain_size, slide, morph, clock] = *c;
        let sr = self.sample_rate;
        let size = self.buf.len() as f64;
        let input = x.clamp(-16.0, 16.0);
        let edge = |gate: bool, prev: &mut bool| {
            let rising = gate && !*prev;
            *prev = gate;
            rising
        };
        let rec_gate = record > 0.5;
        let rec_edge = edge(rec_gate, &mut self.rec_prev);
        let shift_edge = edge(shift > 0.5, &mut self.shift_prev);
        let mark_edge = edge(mark > 0.5, &mut self.mark_prev);
        let unmark_edge = edge(unmark > 0.5, &mut self.unmark_prev);
        let clear_edge = edge(clear > 0.5, &mut self.clear_prev);
        let play_gate = play > 0.5;
        let play_edge = edge(play_gate, &mut self.play_prev);
        if clear_edge {
            self.n = 0;
            self.end = 0.0;
            self.rec = 0;
        }
        // a new splice is finished when its recording stops or the reel is full
        let fin = self.rec == 1 && (!rec_gate || self.w >= size - 1.0);
        if fin {
            let next = (self.n + 1) % self.edges.len();
            self.edges[next] = self.end;
            self.n += 1;
            let newest = (self.n - 1) % self.fresh_at.len();
            self.fresh_at[newest] = true;
        }
        let n = self.n as f64;
        let org = choose.clamp(0.0, 0.9999);
        let changed = (org - self.org_prev).abs() > 0.0001;
        self.org_prev = org;
        self.shift_off = if fin {
            n - 1.0 - (org * n).floor()
        } else if changed {
            0.0
        } else if shift_edge {
            self.shift_off + 1.0
        } else {
            self.shift_off
        };
        let nn = n.max(1.0);
        let pick = (org * n).floor() + self.shift_off;
        let sel = if self.n > 0 { pick - nn * (pick / nn).floor() } else { 0.0 };
        let sel_i = sel.max(0.0) as usize;
        let a = self.edge(sel_i);
        let b = self.edge(sel_i + 1);
        let slen = (b - a).max(2.0);
        let start_new =
            rec_edge && (self.n == 0 || new_splice > 0.5) && self.n < self.splices && self.end < size - 64.0;
        let start_over = rec_edge && !start_new && self.n > 0;
        self.rec = if !rec_gate {
            0
        } else if start_new {
            1
        } else if start_over {
            2
        } else if fin {
            0
        } else {
            self.rec
        };
        if start_new {
            self.w = self.end;
        } else if start_over {
            // just behind the read head, so what is read is the previous pass
            let behind = self.last_pos - 2.0 - a;
            self.w = a + behind - slen * (behind / slen).floor();
            self.oa = a;
            self.ob = b;
        }

        // the window: its length, fade, and how often grains start
        let gs = grain_size.clamp(0.0, 1.0);
        let len = (slen.ln() * (1.0 - gs) + 96f64.ln() * gs).exp().min(slen).max(2.0);
        let whole = len >= slen - 1.0;
        let m = morph.clamp(0.0, 1.0);
        let fade = (len * 0.5).min(0.025 * sr).max(1.0);
        let over = 1.0 + 3.0 * ((m - 0.33) / 0.57).clamp(0.0, 1.0);
        let spacing = if m < 0.33 { len + (0.33 - m) / 0.33 * len } else { len / over }.max(1.0);
        self.speed_now += (speed.clamp(-4.0, 4.0) - self.speed_now) * (1.0 - (-1.0 / (0.02 * sr)).exp());
        let sp = self.speed_now;
        let moving = sp.abs();

        // the clock steps the window one grain at a time, or stretches the splice to its period
        let clk_edge = edge(clock > 0.5, &mut self.clk_prev);
        if clk_edge && self.clk_since > 0.0 {
            self.clk_period = self.clk_since + 1.0;
        }
        self.clk_since = if clk_edge { 0.0 } else { (self.clk_since + 1.0).min(10_000_000.0) };
        let clocked = self.clk_period > 0.0 && self.clk_since < 5.0 * sr;
        let stretch = clocked && m > 0.45;
        let gshift = clocked && m <= 0.45;
        self.cursor = if clk_edge { 0.0 } else { (self.cursor + slen / self.clk_period.max(1.0)).min(slen * 4.0) };
        if clk_edge && gshift {
            self.shift_i += 1.0;
        }
        let base = slide.clamp(0.0, 1.0) * slen;
        let origin = if stretch {
            base + self.cursor
        } else if gshift {
            base + self.shift_i * len
        } else {
            base
        };
        let retrig = play_edge || (clk_edge && gshift);
        let mut slot = None;
        for (i, g) in self.grains.iter_mut().enumerate() {
            if retrig && g.on {
                g.releasing = true;
            }
            if !g.on && slot.is_none() {
                slot = Some(i);
            }
        }
        self.since = (self.since + moving).min(1e9);
        if retrig || self.n == 0 {
            self.fresh = true;
        }
        let spawn = self.n > 0 && b - a > 8.0 && play_gate && slot.is_some() && (self.since >= spacing || self.fresh);
        if spawn {
            self.since = if self.fresh { 0.0 } else { (self.since - spacing).max(0.0) };
            self.fresh = false;
        }
        let r1 = ((self.drawn + 1.0) * 12.9898).sin() * 43758.5453;
        let r1 = r1 - r1.floor();
        let r2 = ((self.drawn + 1.0) * 78.233 + 1.7).sin() * 24634.6345;
        let r2 = r2 - r2.floor();
        let instance = self.drawn - 3.0 * (self.drawn / 3.0).floor();
        let stagger = ((m - 0.45) / 0.15).clamp(0.0, 1.0) * instance / 3.0 * len;
        let jitter = (r1 - 0.5) * 0.08 * len * ((m - 0.33) / 0.57).clamp(0.0, 1.0);
        if spawn {
            self.drawn += 1.0;
        }
        let pan = (r1 * 2.0 - 1.0) * ((m - 0.5) / 0.2).clamp(0.0, 1.0);
        let pitch = 2f64.powf((r2 * 13.0 * ((m - 0.8) / 0.2).clamp(0.0, 1.0)).floor() / 12.0);
        if let (true, Some(i)) = (spawn, slot) {
            self.grains[i] = Grain {
                on: true,
                start: origin + stagger + jitter + if sp < 0.0 { len + fade } else { 0.0 },
                off: 0.0,
                trav: 0.0,
                len: len + fade,
                a,
                sl: slen,
                pitch,
                pan,
                fade,
                whole,
                before: !self.fresh_at[sel_i % self.fresh_at.len()] && a >= 0.01 * sr,
                releasing: false,
                release: 1.0,
            };
        }

        // play the grains
        let (mut l, mut r, mut power, mut sum_gain) = (0.0, 0.0, 0.0, 0.0);
        let mut ended = false;
        let release_step = 1.0 / (0.003 * sr);
        for i in 0..GRAINS {
            let mut g = self.grains[i];
            if !g.on {
                continue;
            }
            let step = moving * g.pitch;
            let t = g.trav + step;
            if g.releasing {
                g.release = (g.release - release_step).max(0.0);
            }
            let done = t >= g.len || (g.releasing && g.release <= 0.0);
            g.off += if sp < 0.0 { -step } else { step };
            g.trav = if done { 0.0 } else { t };
            g.on = !done;
            ended |= done;
            let p = g.start + g.off;
            let gl = g.sl.max(2.0);
            let pi = p.floor();
            let fr = p - pi;
            let wrap = |q: f64| q - gl * (q / gl).floor();
            // the loop point's fades, once per grain (the four taps are within two samples of each other)
            let h = wrap(pi);
            let xf = (0.01 * sr).min(gl / 4.0).max(1.0);
            let hf = (0.002 * sr).min(gl / 4.0).max(1.0);
            let c = 0.5 - 0.5 * (std::f64::consts::PI * ((h - (gl - xf)) / xf).clamp(0.0, 1.0)).cos();
            let fade_in = if g.before { 1.0 } else { 0.5 - 0.5 * (std::f64::consts::PI * (h / hf).min(1.0)).cos() };
            let keep = (1.0 - c) * fade_in;
            let borrow = if g.before { c } else { 0.0 };
            let tap = |q: f64| {
                let h = wrap(q);
                self.sample(g.a + h) * keep + self.sample(g.a + h - gl) * borrow
            };
            let (y0, y1, y2, y3) = (tap(pi - 1.0), tap(pi), tap(pi + 1.0), tap(pi + 2.0));
            let read = y1
                + 0.5 * fr * (y2 - y0 + fr * (2.0 * y0 - 5.0 * y1 + 4.0 * y2 - y3 + fr * (3.0 * (y1 - y2) + y3 - y0)));
            let xw = (t.min(g.len - t) / g.fade.max(1.0)).clamp(0.0, 1.0);
            let win = if g.whole {
                0.5 - 0.5 * (std::f64::consts::PI * xw).cos()
            } else {
                (0.5 * std::f64::consts::PI * xw).sin()
            };
            let gain = if done { 0.0 } else { win * g.release };
            power += gain * gain;
            sum_gain += gain;
            let s = read * gain;
            if !done {
                self.last_pos = g.a + wrap(p);
            }
            l += s * (1.0 - g.pan.max(0.0));
            r += s * (1.0 + g.pan.min(0.0));
            self.grains[i] = g;
        }
        let apart = ((m - 0.45) / 0.15).clamp(0.0, 1.0);
        let norm = (moving * 8.0).min(1.0) / ((1.0 - apart) * sum_gain.max(1.0) + apart * power.max(1.0).sqrt());
        let out_l = (l * norm).clamp(-16.0, 16.0);
        let out_r = (r * norm).clamp(-16.0, 16.0);
        let played = (out_l + out_r) * 0.5;

        // record (the write head moves at normal speed whatever the playback does)
        if self.rec > 0 {
            let mix = sos.clamp(0.0, 1.0);
            let value = (input * (1.0 - mix) + played * mix).clamp(-16.0, 16.0);
            let at = crate::wrapped_index(self.w, self.buf.len());
            self.buf[at] = value as f32;
            let next = self.w + 1.0;
            self.w = if self.rec == 2 && next >= self.ob { self.oa } else { next };
            if self.rec == 1 {
                self.end = self.w;
            }
        }

        // splice markers change only on a mark or unmark edge
        if mark_edge
            && self.n > 0
            && self.n < self.splices
            && self.last_pos - a > 480.0
            && b - self.last_pos > 480.0
        {
            for top in (sel_i + 2..=self.n + 1).rev() {
                self.edges[top] = self.edges[top - 1];
            }
            for top in (sel_i + 2..=self.n).rev() {
                self.fresh_at[top] = self.fresh_at[top - 1];
            }
            self.edges[sel_i + 1] = self.last_pos;
            self.fresh_at[sel_i + 1] = false;
            self.n += 1;
        }
        if unmark_edge && self.n > 1 && sel_i + 1 < self.n {
            for k in sel_i + 1..self.n {
                self.edges[k] = self.edges[k + 1];
            }
            for k in sel_i + 1..self.n - 1 {
                self.fresh_at[k] = self.fresh_at[k + 1];
            }
            self.n -= 1;
        }

        self.eos = if ended { 0.001 * sr } else { (self.eos - 1.0).max(0.0) };
        self.level = played.abs().max(self.level * (1.0 - 1.0 / (0.02 * sr)));
        [
            out_l,
            out_r,
            if self.eos > 0.0 { 1.0 } else { 0.0 },
            self.level.min(1.0),
            sel,
            self.n as f64,
        ]
    }

    fn step(&mut self, _: &ProcessCtx, x: &[f32]) -> [f32; 6] {
        let mut c = [0.0; 14];
        for (value, input) in c.iter_mut().zip(&x[1..]) {
            *value = finite(*input);
        }
        self.tick(finite(x[0]), &c).map(sample)
    }
}
block_opcode!(SpliceReel, "splice_reel_dsp", 6, 15);
