//! Scalar math, comparisons, and interpolated array reads shared by kernels and opcodes.

use crate::opcode::{Opcode, Port, ProcessCtx};
use crate::wrapped_index;

use super::Interp;

/// Scalar math functions, usable inside kernels and as the [`Func`] opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fn1 {
    Sin,
    Cos,
    Tanh,
    Exp,
    Abs,
    Tan,
    Floor,
    Ceil,
    Sqrt,
    Log,
    Sign,
    /// The part after the point: `x - floor(x)`.
    Fract,
}

impl Fn1 {
    /// Every function, for lookup by name.
    pub const ALL: [Fn1; 12] = [
        Fn1::Sin,
        Fn1::Cos,
        Fn1::Tanh,
        Fn1::Exp,
        Fn1::Abs,
        Fn1::Tan,
        Fn1::Floor,
        Fn1::Ceil,
        Fn1::Sqrt,
        Fn1::Log,
        Fn1::Sign,
        Fn1::Fract,
    ];
}

impl Fn1 {
    pub fn apply(self, x: f64) -> f64 {
        match self {
            Fn1::Sin => x.sin(),
            Fn1::Cos => x.cos(),
            Fn1::Tanh => x.tanh(),
            Fn1::Exp => x.exp(),
            Fn1::Abs => x.abs(),
            Fn1::Tan => x.tan(),
            Fn1::Floor => x.floor(),
            Fn1::Ceil => x.ceil(),
            Fn1::Sqrt => x.max(0.0).sqrt(),
            // the natural log, kept finite for a silent or negative input
            Fn1::Log => x.max(1e-30).ln(),
            Fn1::Sign => {
                if x > 0.0 {
                    1.0
                } else if x < 0.0 {
                    -1.0
                } else {
                    0.0
                }
            }
            Fn1::Fract => x - x.floor(),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Fn1::Sin => "sin",
            Fn1::Cos => "cos",
            Fn1::Tanh => "tanh",
            Fn1::Exp => "exp",
            Fn1::Abs => "abs",
            Fn1::Tan => "tan",
            Fn1::Floor => "floor",
            Fn1::Ceil => "ceil",
            Fn1::Sqrt => "sqrt",
            Fn1::Log => "log",
            Fn1::Sign => "sign",
            Fn1::Fract => "fract",
        }
    }
}

/// `f(x)` applied to every sample.
#[derive(Clone)]
pub struct Func(pub Fn1);

impl Opcode for Func {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[Port::required("x")];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(self.clone())
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (o, x) in out.iter_mut().zip(inputs[0]) {
            *o = self.0.apply(f64::from(*x)) as f32;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
    Pow,
    /// The angle of the point (`b`, `a`), in radians.
    Atan2,
    /// The remainder, with the sign of the divisor: `-1.mod(3)` is 2. Zero if the divisor is 0.
    Rem,
}

impl BinOp {
    pub fn apply(self, x: f64, y: f64) -> f64 {
        match self {
            BinOp::Add => x + y,
            BinOp::Sub => x - y,
            BinOp::Mul => x * y,
            BinOp::Div => {
                if y == 0.0 {
                    0.0
                } else {
                    x / y
                }
            }
            BinOp::Min => x.min(y),
            BinOp::Max => x.max(y),
            BinOp::Pow => pow(x, y),
            BinOp::Atan2 => x.atan2(y),
            BinOp::Rem => rem(x, y),
        }
    }
}

/// `pow(x, y)`. Where there is no real answer (a negative base to a fractional power, zero to a negative
/// power) it is 0, as dividing by zero is, so no sound can be made of NaN or infinity this way.
pub fn pow(x: f64, y: f64) -> f64 {
    let r = if x == 0.0 && y < 0.0 { 0.0 } else { x.powf(y) };
    if r.is_nan() { 0.0 } else { r }
}

/// [`pow`] on the engine's own sample type.
pub fn pow32(x: f32, y: f32) -> f32 {
    let r = if x == 0.0 && y < 0.0 { 0.0 } else { x.powf(y) };
    if r.is_nan() { 0.0 } else { r }
}

/// `x.mod(y)`.
pub fn rem(x: f64, y: f64) -> f64 {
    if y == 0.0 {
        0.0
    } else {
        x - y * (x / y).floor()
    }
}

/// A comparison, which gives 1 when it holds and 0 when it doesn't.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cmp {
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
}

impl Cmp {
    pub fn holds(self, x: f64, y: f64) -> bool {
        match self {
            Cmp::Lt => x < y,
            Cmp::Gt => x > y,
            Cmp::Le => x <= y,
            Cmp::Ge => x >= y,
            Cmp::Eq => x == y,
            Cmp::Ne => x != y,
        }
    }
}

/// Read a numeric array through its storage-independent element accessor.
/// This lets tables, buffers and computed arrays share interpolation without
/// gathering their elements or allocating on the audio thread.
#[inline]
pub fn read_array(len: usize, pos: f64, interp: Interp, element: impl Fn(usize) -> f64) -> f64 {
    assert!(len > 0, "cannot read an empty array");
    if !pos.is_finite() {
        return element(0);
    }
    let base = pos.floor();
    let t = pos - base;
    let at = |k: f64| element(wrapped_index(base + k, len));
    match interp {
        Interp::None => element(wrapped_index(base, len)),
        Interp::Linear => {
            if base >= 0.0 && base + 1.0 < len as f64 {
                let i0 = base as usize;
                let y0 = element(i0);
                let y1 = element(i0 + 1);
                return y0 + (y1 - y0) * t;
            }
            let y0 = at(0.0);
            let y1 = at(1.0);
            y0 + (y1 - y0) * t
        }
        Interp::Cubic => {
            let (y0, y1, y2, y3) = (at(-1.0), at(0.0), at(1.0), at(2.0));
            let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
            let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let c = -0.5 * y0 + 0.5 * y2;
            ((a * t + b) * t + c) * t + y1
        }
    }
}

#[inline]
pub fn read_at(data: &[f64], pos: f64, interp: Interp) -> f64 {
    read_array(data.len(), pos, interp, |i| data[i])
}
