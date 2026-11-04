//! Tempo curves, value arithmetic, event parsing and converting sections to clips.

use super::*;

/// How long, in bars, each step of a gradual tempo change is.
pub(super) const RAMP_STEP_BARS: f64 = 1.0 / 8.0;

/// A tempo over the piece, in bars of music: `env[84bpm 64bars 84bpm 8bars.log 96bpm]`.
pub(super) struct TempoCurve {
    pub(super) start: f64,
    /// For each stretch: how many bars it takes, its shape, and the tempo it reaches.
    pub(super) segments: Vec<(f64, Curve, f64)>,
}

impl TempoCurve {
    /// The tempo `bar` bars into the piece, holding the last level after the end.
    pub(super) fn bpm_at(&self, bar: f64) -> f64 {
        let (mut from, mut at) = (self.start, 0.0);
        for &(len, shape, to) in &self.segments {
            if bar < at + len {
                let t = ((bar - at) / len).clamp(0.0, 1.0);
                return f64::from(niminal_engine::ops::shape(
                    from as f32,
                    to as f32,
                    t,
                    shape.bend(),
                ));
            }
            (from, at) = (to, at + len);
        }
        from
    }
}

pub(super) fn tempo_curve(lit: &EnvLit) -> Res<TempoCurve> {
    let bpm = |v: &EnvValue| {
        let n = lit_step(v)?;
        match (n.unit.as_deref(), n.value) {
            (Some("bpm"), v) if v > 0.0 => Ok(v),
            _ => Err(
                Diagnostic::new("a tempo level is a positive number of bpm", n.span)
                    .with_help("for example `env[84bpm 8bars 96bpm]`"),
            ),
        }
    };
    if lit.sustain_at.is_some() || lit.looped {
        return Err(Diagnostic::new(
            "a tempo envelope runs once over the piece",
            lit.start.span(),
        )
        .with_help("take out the `|` or `.loop`"));
    }
    let mut segments = Vec::with_capacity(lit.segments.len());
    for seg in &lit.segments {
        let d = lit_step(&seg.dur)?;
        let len = match (d.unit.as_deref(), d.value) {
            (Some("bar" | "bars"), v) if v > 0.0 => v,
            _ => {
                return Err(Diagnostic::new(
                    "the times in a tempo envelope are lengths in bars",
                    d.span,
                )
                .with_help("for example `8bars`"));
            }
        };
        segments.push((len, curve_of(seg.curve), bpm(&seg.level)?));
    }
    Ok(TempoCurve {
        start: bpm(&lit.start)?,
        segments,
    })
}

pub(crate) fn negate(v: Value) -> Value {
    match v {
        Value::Num(n) => Value::Num(-n),
        Value::Hz(n) => Value::Hz(-n),
        Value::Db(n) => Value::Db(-n),
        Value::Seconds(n) => Value::Seconds(-n),
        Value::Beats(n) => Value::Beats(-n),
        Value::Bars(n) => Value::Bars(-n),
        Value::Semitones(n) => Value::Semitones(-n),
        Value::Degrees(n) => Value::Degrees(-n),
        keep @ (Value::Note(_) | Value::Sample(_)) => keep,
    }
}

pub(super) fn op_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "subtract",
        BinOp::Mul => "multiply",
        BinOp::Div => "divide",
    }
}

pub(super) fn cmp_values(op: CmpOp, a: Value, b: Value) -> bool {
    let key = |v: Value| match v {
        Value::Note(n) => n as f64,
        Value::Hz(h) => h,
        Value::Num(n) => n,
        Value::Db(d) => d,
        Value::Seconds(s) => s,
        Value::Beats(b) => b,
        Value::Bars(b) => b,
        Value::Semitones(s) => s,
        Value::Degrees(d) => d,
        Value::Sample(s) => s as f64,
    };
    let (a, b) = (key(a), key(b));
    match op {
        CmpOp::Lt => a < b,
        CmpOp::Gt => a > b,
        CmpOp::Le => a <= b,
        CmpOp::Ge => a >= b,
        CmpOp::Eq => (a - b).abs() < 1e-12,
        CmpOp::Ne => (a - b).abs() >= 1e-12,
    }
}

/// One note's value combined with another's. A plain number added to a note moves it
/// by that many semitones, so `[c4 e4] + [0 7]` works. `None` if the units don't mix,
/// which leaves the note as it was.
pub(super) fn step_arithmetic(op: BinOp, a: Value, b: Value) -> Option<Value> {
    let (a, b) = match (op, a, b) {
        (BinOp::Add | BinOp::Sub, Value::Note(_), Value::Num(n)) => (a, Value::Semitones(n)),
        (BinOp::Add, Value::Num(n), Value::Note(_)) => (Value::Semitones(n), b),
        _ => (a, b),
    };
    let at = Span::default();
    match op {
        BinOp::Add => add_values(a, b, at),
        BinOp::Sub => sub_values(a, b, at),
        BinOp::Mul => mul_values(a, b, at),
        BinOp::Div => div_values(a, b, at),
    }
    .ok()
}

pub(super) fn add_values(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Note(n), Value::Semitones(st)) | (Value::Semitones(st), Value::Note(n)) => {
            Ok(transpose(Value::Note(n), st))
        }
        (Value::Hz(h), Value::Semitones(st)) | (Value::Semitones(st), Value::Hz(h)) => {
            Ok(transpose(Value::Hz(h), st))
        }
        (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a + b)),
        (Value::Db(a), Value::Db(b)) => Ok(Value::Db(a + b)),
        (Value::Beats(a), Value::Beats(b)) => Ok(Value::Beats(a + b)),
        (Value::Bars(a), Value::Bars(b)) => Ok(Value::Bars(a + b)),
        (Value::Seconds(a), Value::Seconds(b)) => Ok(Value::Seconds(a + b)),
        (Value::Semitones(a), Value::Semitones(b)) => Ok(Value::Semitones(a + b)),
        (Value::Hz(a), Value::Hz(b)) => Ok(Value::Hz(a + b)),
        _ => Err(Diagnostic::new("can't add these units", span)),
    }
}

pub(super) fn sub_values(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Note(n), Value::Semitones(st)) => Ok(transpose(Value::Note(n), -st)),
        (Value::Hz(h), Value::Semitones(st)) => Ok(transpose(Value::Hz(h), -st)),
        (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a - b)),
        (Value::Db(a), Value::Db(b)) => Ok(Value::Db(a - b)),
        (Value::Beats(a), Value::Beats(b)) => Ok(Value::Beats(a - b)),
        (Value::Bars(a), Value::Bars(b)) => Ok(Value::Bars(a - b)),
        (Value::Seconds(a), Value::Seconds(b)) => Ok(Value::Seconds(a - b)),
        (Value::Semitones(a), Value::Semitones(b)) => Ok(Value::Semitones(a - b)),
        (Value::Hz(a), Value::Hz(b)) => Ok(Value::Hz(a - b)),
        _ => Err(Diagnostic::new("can't subtract these units", span)),
    }
}

pub(super) fn mul_values(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a * b)),
        (v, Value::Num(n)) | (Value::Num(n), v) => match v {
            Value::Beats(x) => Ok(Value::Beats(x * n)),
            Value::Bars(x) => Ok(Value::Bars(x * n)),
            Value::Seconds(x) => Ok(Value::Seconds(x * n)),
            Value::Hz(x) => Ok(Value::Hz(x * n)),
            Value::Db(x) => Ok(Value::Db(x * n)),
            Value::Semitones(x) => Ok(Value::Semitones(x * n)),
            Value::Num(x) => Ok(Value::Num(x * n)),
            _ => Err(Diagnostic::new("can't multiply these units", span)),
        },
        _ => Err(Diagnostic::new("can't multiply these units", span)),
    }
}

pub(super) fn div_values(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Num(a), Value::Num(b)) if b != 0.0 => Ok(Value::Num(a / b)),
        (v, Value::Num(n)) if n != 0.0 => match v {
            Value::Beats(x) => Ok(Value::Beats(x / n)),
            Value::Bars(x) => Ok(Value::Bars(x / n)),
            Value::Seconds(x) => Ok(Value::Seconds(x / n)),
            Value::Hz(x) => Ok(Value::Hz(x / n)),
            Value::Num(x) => Ok(Value::Num(x / n)),
            _ => Err(Diagnostic::new("can't divide these units", span)),
        },
        _ => Err(Diagnostic::new("can't divide these values", span)),
    }
}

/// Move a pitch by `st` semitones. Notes stay notes when the interval is whole.
pub(crate) fn transpose_public(v: Value, st: f64) -> Value {
    transpose(v, st)
}

pub(super) fn transpose(v: Value, st: f64) -> Value {
    match v {
        Value::Note(m) if st.fract() == 0.0 => Value::Note(m.saturating_add(st as i32)),
        Value::Note(_) | Value::Hz(_) => {
            let hz = v.as_hz().expect("notes and frequencies have a frequency");
            Value::Hz(hz * 2f64.powf(st / 12.0))
        }
        other => other,
    }
}

/// Time as a number of cycles (bars) at `tempo`.
pub(super) fn time_cycles(t: Time, tempo: Tempo) -> Rational {
    let beats = match t {
        Time::Bars(b) => b * tempo.beats_per_bar,
        Time::Beats(b) => b,
        Time::Seconds(s) => s * tempo.bpm / 60.0,
    };
    Rational::from_f64(beats / tempo.beats_per_bar)
}

/// `events { 0beat c4 for 1/2beat gain: -6db }` inside a clip.
pub(super) fn parse_events(raw: &str, span: Span, cx: &Context) -> Res<Vec<ScoredNote>> {
    let mut out = Vec::new();
    for (line_i, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let err = |message: String| {
            Diagnostic::new(message, span).with_help(format!("on line {}: `{line}`", line_i + 1))
        };
        // `<at> <value> for <dur> [name: value …]`
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 4 || tokens[2] != "for" {
            return Err(err(
                "an event is `at value for dur`, as in `0beat c4 for 1/2beat`".into(),
            ));
        }
        let at: Time = tokens[0]
            .parse()
            .map_err(|e: niminal_score::ParseError| err(e.to_string()))?;
        let value = atom_value(tokens[1], cx.sample_names).map_err(err)?;
        let dur: Time = tokens[3]
            .parse()
            .map_err(|e: niminal_score::ParseError| err(e.to_string()))?;
        let mut step = Step::from(value);
        let mut args = Vec::new();
        let mut i = 4;
        while i < tokens.len() {
            let part = tokens[i];
            let (name, val_tok) = if let Some((n, v)) = part.split_once(':') {
                if v.is_empty() {
                    i += 1;
                    if i >= tokens.len() {
                        return Err(err(format!("`{n}:` needs a value")));
                    }
                    (n, tokens[i])
                } else {
                    (n, v)
                }
            } else if let Some(n) = part.strip_suffix(':') {
                i += 1;
                if i >= tokens.len() {
                    return Err(err(format!("`{n}:` needs a value")));
                }
                (n, tokens[i])
            } else {
                return Err(err(format!(
                    "expected `name: value` after the duration, found `{part}`"
                )));
            };
            let v = atom_value(val_tok, cx.sample_names).map_err(err)?;
            match name {
                "gain" => {
                    step.gain = Some(match v {
                        Value::Db(db) => db,
                        other => other.as_gain().map_err(|e| err(e.to_string()))?.log10() * 20.0,
                    });
                }
                "pan" => {
                    step.pan = Some(v.as_angle().map_err(|e| err(e.to_string()))?);
                }
                "begin" => {
                    step.begin = Some(v.as_number().map_err(|e| err(e.to_string()))?);
                }
                "pitch" => {
                    step.pitch = Some(v.as_semitones().map_err(|e| err(e.to_string()))?);
                }
                other => args.push((other.to_string(), v)),
            }
            i += 1;
        }
        out.push(ScoredNote {
            at: time_cycles(at, cx.tempo),
            dur: time_cycles(dur, cx.tempo),
            step,
            args,
        });
    }
    Ok(out)
}

pub(super) fn section_to_clip(section: crate::Section, tempo: Tempo) -> Res<Clip> {
    let length = time_cycles(section.length, tempo);
    let mut scored = Vec::with_capacity(section.events.len());
    for e in section.events {
        let mut step = Step::from(Value::Num(0.0));
        let mut args = Vec::new();
        for (k, v) in e.args {
            match k.as_str() {
                "freq" | "note" => step = Step::from(v),
                "gain" | "amp" => {
                    step.gain = match v {
                        Value::Db(db) => Some(db),
                        other => Some(
                            other
                                .as_gain()
                                .map_err(|err| Diagnostic::new(err.to_string(), Span::new(0, 0)))?
                                .log10()
                                * 20.0,
                        ),
                    };
                }
                "pan" => {
                    step.pan = Some(
                        v.as_angle()
                            .map_err(|err| Diagnostic::new(err.to_string(), Span::new(0, 0)))?,
                    );
                }
                other => args.push((other.to_string(), v)),
            }
        }
        if matches!(step.value, Value::Num(0.0)) && args.is_empty() && step.gain.is_none() {
            // no pitch: skip empty shell events that only name the target
            continue;
        }
        scored.push(ScoredNote {
            at: time_cycles(e.at, tempo),
            dur: time_cycles(e.dur, tempo),
            step,
            args,
        });
    }
    Ok(Clip {
        name: section.name,
        length: if length > Rational::ZERO {
            length
        } else {
            Rational::ONE
        },
        endless: false,
        plays: None,
        then: Vec::new(),
        lanes: Vec::new(),
        scored,
        keys: None,
        curves: Vec::new(),
        signals: Vec::new(),
        fx_params: Vec::new(),
    })
}
