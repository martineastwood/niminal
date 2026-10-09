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
    /// Scale `x` by a linear gain factor. Decibels are converted before the
    /// graph is built.
    Gain, "gain", "x", "gain", Port::optional("gain", 1.0), |x, g| x * g
);
