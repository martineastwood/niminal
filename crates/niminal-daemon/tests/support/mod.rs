//! What the engine's tests share: putting a project in place the way a client of the engine would.

#![allow(dead_code)]

use niminal_daemon::{At, Op, Problem, Session};
use niminal_lang::{StatementKind, analyze, compile};
use niminal_score::Time;

/// Put a project in place at once, with the engine's own events: its definitions with `define`, its tempo
/// with `transport`, and the notes it writes out with `note`.
pub trait Eval {
    fn eval(&mut self, source: &str, quantize: Option<&str>) -> Result<(), Vec<Problem>>;

    /// Like `eval`, and the notes its clips play over the next `seconds` as well.
    fn eval_for(&mut self, source: &str, seconds: f64) -> Result<(), Vec<Problem>>;
}

impl Eval for Session {
    fn eval(&mut self, source: &str, _: Option<&str>) -> Result<(), Vec<Problem>> {
        install(self, source, 0.0)
    }

    fn eval_for(&mut self, source: &str, seconds: f64) -> Result<(), Vec<Problem>> {
        install(self, source, seconds)
    }
}

fn install(session: &mut Session, source: &str, play_seconds: f64) -> Result<(), Vec<Problem>> {
    let program = compile(source).map_err(|ds| ds.into_iter().map(|d| Problem::plain(d.message)).collect::<Vec<_>>())?;
    let statements = analyze(source).map_err(|d| vec![Problem::plain(d.message)])?;
    let mut ops = Vec::new();
    let mut definitions = Vec::new();
    for s in &statements {
        match (s.kind(), s.key()) {
            (StatementKind::Definition, Some("tempo")) => {
                ops.push(Op::Transport { at: At::Now, bpm: Some(program.tempo.bpm), meter: None, origin: None, tag: None });
            }
            (StatementKind::Definition, Some("meter")) => {
                let meter = s.text.trim().strip_prefix("meter").unwrap_or(&s.text).trim().to_string();
                ops.push(Op::Transport { at: At::Now, bpm: None, meter: Some(meter), origin: None, tag: None });
            }
            (StatementKind::Definition, _) => definitions.push(s.text.clone()),
            _ => {}
        }
    }
    if !definitions.is_empty() {
        ops.insert(0, Op::Define { source: definitions.join("\n"), at: At::Now, dir: None, tag: None });
    }
    // notes written out in the code, and what clips play
    let start = session.clock();
    let rate = f64::from(session.sample_rate());
    let sample = |seconds: f64| At::Sample(start + (seconds * rate).round() as u64);
    let mut events = program.notes.clone();
    if play_seconds > 0.0 {
        events.extend(program.schedule().events(0.0, play_seconds).map_err(|e| vec![Problem::plain(e.to_string())])?);
    }
    for e in events {
        let at = sample(e.at.to_seconds(program.tempo));
        ops.push(Op::Note { target: e.target, at, args: e.args, dur: Some(Time::Seconds(e.dur.to_seconds(program.tempo))), id: None, tag: None });
    }
    session.send_events(0, ops).map(|_| ())
}

/// Move a control at sample `at` (now if none), over `smooth_ms` (the control's own smoothing if none).
pub fn set_control(
    session: &mut Session,
    name: &str,
    value: f64,
    unit: Option<&str>,
    at: Option<u64>,
    smooth_ms: Option<f64>,
) -> Result<(), Vec<Problem>> {
    let op = Op::Control {
        name: name.into(),
        at: at.map_or(At::Now, At::Sample),
        value,
        unit: unit.map(str::to_owned),
        glide: smooth_ms.map(|ms| Time::Seconds(ms / 1000.0)),
        tag: None,
    };
    session.send_events(0, vec![op]).map(|_| ())
}
