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
