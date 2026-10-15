use crate::opcode::{Opcode, Port, ProcessCtx};

macro_rules! binary_op {
    ($(#[$doc:meta])* $name:ident, $label:literal, $a:literal, $b:literal, $default_b:expr, |$x:ident, $y:ident| $body:expr) => {
        $(#[$doc])*
        #[derive(Clone, Default)]
        pub struct $name;

        impl Opcode for $name {
            fn name(&self) -> &'static str {
                $label
            }

            fn ports(&self) -> &'static [Port] {
                const PORTS: &[Port] = &[Port::required($a), $default_b];
                PORTS
            }

            fn box_clone(&self) -> Box<dyn Opcode> {
                Box::new(self.clone())
            }

            fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
                for (i, o) in out.iter_mut().enumerate() {
                    let ($x, $y) = (inputs[0][i], inputs[1][i]);
                    *o = $body;
                }
            }
        }
    };
}

binary_op!(
    /// `a + b`
    Add, "add", "a", "b", Port::required("b"), |a, b| a + b
);
binary_op!(
    /// `a - b`
    Sub, "sub", "a", "b", Port::required("b"), |a, b| a - b
);
binary_op!(
    /// `a * b`
    Mul, "mul", "a", "b", Port::required("b"), |a, b| a * b
);
binary_op!(
    /// `a / b`, which is 0 where `b` is 0
    Div, "div", "a", "b", Port::required("b"), |a, b| if b == 0.0 { 0.0 } else { a / b }
);
binary_op!(
    /// `a % b`, with the sign of `b`
    Rem, "rem", "a", "b", Port::required("b"), |a, b| super::custom::rem(f64::from(a), f64::from(b)) as f32
);

/// A comparison, 1 where it holds and 0 where it doesn't.
#[derive(Clone)]
pub struct Compare(pub super::custom::Cmp);

impl Opcode for Compare {
    fn name(&self) -> &'static str {
        "compare"
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[Port::required("a"), Port::required("b")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(self.clone())
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            *o = if self.0.holds(f64::from(inputs[0][i]), f64::from(inputs[1][i])) { 1.0 } else { 0.0 };
        }
    }
}

/// `a` where `cond` is not 0, `b` elsewhere.
#[derive(Clone, Default)]
pub struct Select;

impl Opcode for Select {
    fn name(&self) -> &'static str {
        "select"
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[Port::required("cond"), Port::required("a"), Port::required("b")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(self.clone())
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            *o = if inputs[0][i] != 0.0 { inputs[1][i] } else { inputs[2][i] };
        }
    }
}

binary_op!(
    /// Scale `x` by a linear gain factor. Decibels are converted before the
    /// graph is built.
    Gain, "gain", "x", "gain", Port::optional("gain", 1.0), |x, g| x * g
);

binary_op!(
    /// The smaller of `a` and `b`
    Min, "min", "a", "b", Port::required("b"), |a, b| a.min(b)
);
binary_op!(
    /// The larger of `a` and `b`
    Max, "max", "a", "b", Port::required("b"), |a, b| a.max(b)
);
binary_op!(
    /// `a` raised to `b`
    Pow, "pow", "a", "b", Port::required("b"), |a, b| a.powf(b)
);

/// `x.init`: the value `x` had when the note started, held for the whole note.
#[derive(Clone, Default)]
pub struct Hold {
    value: Option<f32>,
}

impl Opcode for Hold {
    fn reset(&mut self) { self.value = None; }
    fn name(&self) -> &'static str {
        "hold"
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[Port::required("x")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Hold::default())
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.value = old.value;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        if out.is_empty() {
            return;
        }
        let v = *self.value.get_or_insert(inputs[0][0]);
        out.fill(v);
    }
}

/// `x.control`: `x` read at the start of each block and held through it.
#[derive(Clone, Default)]
pub struct BlockHold;

impl Opcode for BlockHold {
    fn name(&self) -> &'static str {
        "block_hold"
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[Port::required("x")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(BlockHold)
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        if let Some(&v) = inputs[0].first() {
            out.fill(v);
        }
    }
}

/// `prev(x)`: what `x` was one sample earlier.
#[derive(Clone, Default)]
pub struct Delay1 {
    last: f32,
}

impl Opcode for Delay1 {
    fn reset(&mut self) { self.last = 0.0; }
    fn name(&self) -> &'static str {
        "prev"
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[Port::required("x")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Delay1::default())
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.last = old.last;
        }
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (o, x) in out.iter_mut().zip(inputs[0]) {
            *o = self.last;
            self.last = *x;
        }
    }
}

/// A table read at a position: whole entries with `Interp::None`, or between them. Wraps.
#[derive(Clone)]
pub struct TableRead {
    data: std::sync::Arc<crate::Array<f64>>,
    interp: super::custom::Interp,
}

impl TableRead {
    pub fn new(data: std::sync::Arc<crate::Array<f64>>, interp: super::custom::Interp) -> Self {
        assert!(!data.is_empty(), "a table has at least one entry");
        TableRead { data, interp }
    }
}

impl Opcode for TableRead {
    fn name(&self) -> &'static str {
        "table"
    }

    fn ports(&self) -> &'static [Port] {
        const PORTS: &[Port] = &[Port::required("pos")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(self.clone())
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (o, p) in out.iter_mut().zip(inputs[0]) {
            *o = super::custom::read_at(&self.data, f64::from(*p), self.interp) as f32;
        }
    }
}

/// Seconds since the note started.
#[derive(Clone, Default)]
pub struct NoteTime {
    samples: u64,
    sample_rate: f32,
}

impl Opcode for NoteTime {
    fn reset(&mut self) { self.samples = 0; }
    fn name(&self) -> &'static str {
        "note_time"
    }

    fn ports(&self) -> &'static [Port] {
        &[]
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(NoteTime::default())
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    fn carry_state(&mut self, old: &mut dyn Opcode) {
        if let Some(old) = (old as &mut dyn std::any::Any).downcast_mut::<Self>() {
            self.samples = old.samples;
        }
    }

    fn process(&mut self, ctx: &ProcessCtx, _inputs: &[&[f32]], out: &mut [f32]) {
        for o in out.iter_mut() {
            *o = (self.samples as f64 / f64::from(ctx.sample_rate)) as f32;
            self.samples += 1;
        }
    }
}

/// 0 while the note is held, 1 once it has been released.
#[derive(Clone, Default)]
pub struct Released;

impl Opcode for Released {
    fn name(&self) -> &'static str {
        "released"
    }

    fn ports(&self) -> &'static [Port] {
        &[]
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(Released)
    }

    fn process(&mut self, ctx: &ProcessCtx, _inputs: &[&[f32]], out: &mut [f32]) {
        out.fill(if ctx.gate { 0.0 } else { 1.0 });
    }
}

/// The sample rate, in hz.
#[derive(Clone, Default)]
pub struct SampleRate;

impl Opcode for SampleRate {
    fn name(&self) -> &'static str {
        "sample_rate"
    }

    fn ports(&self) -> &'static [Port] {
        &[]
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(SampleRate)
    }

    fn process(&mut self, ctx: &ProcessCtx, _inputs: &[&[f32]], out: &mut [f32]) {
        out.fill(ctx.sample_rate);
    }
}

/// A numeric array of computed values, read by position without a linear
/// chain of switches. Graph slots already hold the elements.
#[derive(Clone)]
pub struct ArrayRead {
    ports: Vec<Port>,
    interp: super::custom::Interp,
}

impl ArrayRead {
    pub fn new(len: usize, interp: super::custom::Interp) -> Self {
        assert!(len > 0);
        let ports = std::iter::once(Port::named("pos", None))
            .chain((0..len).map(|i| Port::named(format!("e{i}"), None))).collect();
        Self { ports, interp }
    }
}

impl Opcode for ArrayRead {
    fn name(&self) -> &str { "array_read" }
    fn ports(&self) -> &[Port] { &self.ports }
    fn box_clone(&self) -> Box<dyn Opcode> { Box::new(self.clone()) }
    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, value) in out.iter_mut().enumerate() {
            *value = super::custom::read_array(inputs.len() - 1, f64::from(inputs[0][i]), self.interp,
                |k| f64::from(inputs[k + 1][i])) as f32;
        }
    }
    fn max_inputs(&self) -> usize { self.ports().len() }

    fn process_slots(&mut self, _ctx: &ProcessCtx, slots: &[[f32; crate::BLOCK]], indices: &[usize], outs: &mut [[f32; crate::BLOCK]], frames: usize) {
        for i in 0..frames {
            outs[0][i] = super::custom::read_array(indices.len() - 1, f64::from(slots[indices[0]][i]), self.interp,
                |k| f64::from(slots[indices[k + 1]][i])) as f32;
        }
    }
}
