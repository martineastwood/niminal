//! What the engine's tests share: putting definitions in place the way a client of the engine would.

#![allow(dead_code)]

use niminal_server::{At, Op, Problem, Session};
use niminal_score::Time;

/// Put definitions in place at once, with the engine's own `define` event.
pub trait Eval {
    fn eval_as(&mut self, client: u64, source: &str) -> Result<(), Vec<Problem>>;
    fn eval(&mut self, source: &str, quantize: Option<&str>) -> Result<(), Vec<Problem>>;

    /// Like `eval`, and the notes its clips play over the next `seconds` as well.
    fn eval_for(&mut self, source: &str, seconds: f64) -> Result<(), Vec<Problem>>;
}

impl Eval for Session {
    fn eval_as(&mut self, client: u64, source: &str) -> Result<(), Vec<Problem>> {
        install_as(self, source, client)
    }
    fn eval(&mut self, source: &str, _: Option<&str>) -> Result<(), Vec<Problem>> {
        install(self, source, 0.0)
    }

    fn eval_for(&mut self, source: &str, seconds: f64) -> Result<(), Vec<Problem>> {
        install(self, source, seconds)
    }
}

fn install(session: &mut Session, source: &str, _play_seconds: f64) -> Result<(), Vec<Problem>> {
    install_as(session, source, 0)
}

fn install_as(session: &mut Session, source: &str, client: u64) -> Result<(), Vec<Problem>> {
    let ops = vec![Op::Define {
        source: source.to_string(),
        at: At::Now,
        dir: None,
        tag: None,
    }];
    session.send_events(client, ops).map(|_| ())
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

/// Play `target` (an instrument or node, no arguments) `at` seconds from now, for `dur` seconds.
pub fn play(session: &mut Session, target: &str, at: f64, dur: f64) {
    let start = session.clock() + (at * f64::from(session.sample_rate())).round() as u64;
    let op = Op::Note {
        to: None,
        target: target.into(),
        at: At::Sample(start),
        args: Default::default(),
        dur: Some(Time::Seconds(dur)),
        id: None,
        tag: None,
    };
    session.send_events(0, vec![op]).unwrap();
}
