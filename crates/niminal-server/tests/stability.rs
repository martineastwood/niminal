//! Every effect on the std shelf stays finite whatever its controls are set to, and an effect that
//! was silenced for going wrong plays again once it is redefined.

use niminal_lang::Layout;
use niminal_lang::presets::{PresetKind, of_shelf};
use niminal_score::Time;
use niminal_server::{At, Op, Session};

fn define(source: &str) -> Op {
    Op::Define {
        source: source.into(),
        at: At::Now,
        dir: None,
        tag: None,
    }
}

fn note() -> Op {
    Op::Note {
        target: "n".into(),
        to: None,
        at: At::Now,
        args: Default::default(),
        dur: Some(Time::Seconds(1.0)),
        id: None,
        tag: None,
    }
}

/// The audio from a saw with noise on top played through `fx_source`, as the effect `fx`, for `frames`.
fn through(session: &mut Session, fx_source: &str, fx: &str, frames: usize) -> Option<Vec<f32>> {
    let source = format!(
        "{fx_source}\ninstr src() {{ osc(saw, 220hz) * 0.4 + noise() * 0.2 }}\nnode n {{ instrument = src\n output = {fx} }}"
    );
    // a definition the server refuses out loud is not a failure
    session.send_events(1, vec![define(&source), note()]).ok()?;
    Some(session.process(frames).remove(0))
}

/// The server takes seconds, not beats.
fn at_120bpm(text: &str) -> String {
    text.replace("3/4beat", "375ms")
        .replace("1/2beat", "250ms")
        .replace("1/4beat", "125ms")
        .replace("1bar", "2sec")
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |a, b| f32::max(a, b.abs()))
}

/// `label` ("cutoff: 20hz..20000hz = 2400hz") with its default replaced by `factor` times it, if it
/// has a number for a default.
fn scaled(label: &str, factor: f64) -> Option<String> {
    let (head, default) = label.rsplit_once(" = ")?;
    let end = default
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(default.len());
    let number: f64 = default[..end].parse().ok()?;
    Some(format!("{head} = {}{}", number * factor, &default[end..]))
}

#[test]
fn shelf_effects_survive_extreme_controls() {
    let (mut failures, mut played, mut refused) = (Vec::new(), 0, 0);
    for preset in of_shelf("std/fx").filter(|p| p.kind == PresetKind::Effect) {
        for factor in [0.0, -1.0, 1e3, 1e5, 1e9] {
            let mut source = at_120bpm(&format!("{}\n{}", preset.helpers, preset.source));
            for label in preset.params.iter().map(|l| at_120bpm(l)) {
                if let Some(extreme) = scaled(&label, factor) {
                    source = source.replace(&label, &extreme);
                }
            }
            let mut session = Session::new(48_000.0, Layout::Mono).without_limiter();
            let Some(audio) = through(&mut session, &source, &preset.id, 48_000) else {
                refused += 1;
                continue;
            };
            played += 1;
            let silenced = session.take_silenced();
            if silenced > 0 || audio.iter().any(|v| !v.is_finite()) {
                failures.push(format!(
                    "{} with controls scaled by {factor}: {silenced} silenced, finite {}",
                    preset.id,
                    audio.iter().all(|v| v.is_finite())
                ));
            }
        }
    }
    eprintln!("{played} effect settings played, {refused} refused by the server");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(played > refused, "too few settings were actually played");
}

#[test]
fn an_effect_silenced_for_going_wrong_plays_again_when_redefined() {
    let mut session = Session::new(48_000.0, Layout::Mono).without_limiter();
    let broken = through(
        &mut session,
        "fx wreck { out = exp(in * 1000 + 1000) }",
        "wreck",
        9_600,
    )
    .unwrap();
    assert_eq!(peak(&broken), 0.0);
    assert!(
        session.take_silenced() > 0,
        "the broken effect was not noticed"
    );

    let fixed = through(&mut session, "fx wreck { out = in * 0.5 }", "wreck", 9_600).unwrap();
    assert!(
        peak(&fixed) > 0.05,
        "the redefined effect stays silent: {}",
        peak(&fixed)
    );
    assert_eq!(session.take_silenced(), 0);
}

/// What a default session plays, which is what the live audio device is given, can never be louder than the
/// limiter's ceiling, whatever the piece does.
#[test]
fn a_default_session_cannot_be_louder_than_the_limiter_ceiling() {
    let mut session = Session::new(48_000.0, Layout::Mono);
    let source = "instr loud() { osc(saw, 110hz) * 1000000 }\nnode n { instrument = loud }";
    session
        .send_events(1, vec![define(source), note()])
        .unwrap();
    let audio = session.process(48_000).remove(0);
    assert!(
        peak(&audio) > 0.5,
        "the test must actually be loud: {}",
        peak(&audio)
    );
    assert!(
        peak(&audio) <= niminal_engine::DEFAULT_CEILING + 1e-4,
        "{}",
        peak(&audio)
    );
    assert!(audio.iter().all(|v| v.is_finite()));
}
