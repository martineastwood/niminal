use niminal_server::{At, EngineServer, LogInput, Op, Session};
use niminal_lang::Layout;
use niminal_live::{InProcess, Live, Offline};
use std::sync::{Arc, Mutex};

#[test]
fn composition_is_lowered_to_physical_events_before_it_reaches_the_engine() {
    let mut s = Offline::new(48_000.0, Layout::Mono).without_limiter();
    s.eval("tempo 90bpm\nmeter 3/4\nctl level = midi.cc(7).range(0..1).smooth(10ms)\ninstr x(freq: hz, decay: sec = 1/4beat) { quantize = 1bar\n osc(sine, freq) * env[1 | decay 0] }\nclip riff { instrument: x\nnotes: [c4 e4 g4]\nlength: 1bar }\nscene verse { riff }\nverse.start", Some("now")).unwrap();
    s.process(24_000);
    assert!(s.take_notices().is_empty());
    let mut notes = 0;
    for entry in s.log() {
        if let LogInput::Send { ops, .. } = entry.input {
            for op in ops {
                match op {
                    Op::Define { source, .. } => niminal_lang::validate_dsp(&source).unwrap(),
                    Op::Note { at, dur, .. } => {
                        assert!(matches!(at, At::Sample(_)));
                        assert!(matches!(dur, Some(niminal_score::Time::Seconds(_))));
                        notes += 1;
                    }
                    _ => {}
                }
            }
        }
    }
    assert!(notes > 0);
}

#[test]
fn two_live_clients_keep_independent_tempos_and_musical_clock_controls() {
    let engine = Arc::new(Mutex::new(EngineServer::new(
        Session::new(48_000.0, Layout::Mono).without_limiter(),
    )));
    let mut a = Live::new(
        Box::new(InProcess::new(engine.clone())),
        48_000.0,
        Layout::Mono,
    );
    let mut b = Live::new(
        Box::new(InProcess::new(engine.clone())),
        48_000.0,
        Layout::Mono,
    );
    a.eval(
        "tempo 60bpm\ninstr a() { tail = 0ms\n clock.bpm * 0.001 }\na() for 2sec",
        Some("now"),
    )
    .unwrap();
    b.eval(
        "tempo 240bpm\ninstr b() { tail = 0ms\n clock.bpm * 0.001 }\nb() for 2sec",
        Some("now"),
    )
    .unwrap();
    a.advance();
    b.advance();
    let first = engine.lock().unwrap().session_mut().process(4800).remove(0);
    assert!((first[4000] - 0.3).abs() < 1e-6, "{}", first[4000]);
    a.eval("tempo 120bpm", Some("now")).unwrap();
    a.advance();
    b.advance();
    let second = engine.lock().unwrap().session_mut().process(4800).remove(0);
    assert!((second[4000] - 0.36).abs() < 1e-6, "{}", second[4000]);
    assert_eq!(a.transport().bpm, 120.0);
    assert_eq!(b.transport().bpm, 240.0);
    assert!(a.take_notices().is_empty());
    assert!(b.take_notices().is_empty());
}
