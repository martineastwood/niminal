pub use niminal_daemon::{At, Op, QuantizeDefaults, Session, from_lines, to_lines};
pub use niminal_lang::Layout;
pub use niminal_score::{Time, Value};

pub const SR: f32 = 48_000.0;
pub const BAR: usize = 96_000; // 120bpm in 4/4

pub const SETUP: &str = "
tempo 120bpm
instr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }
track lead { instrument = pluck }
track bass { instrument = pluck }
";

pub fn session() -> Session {
    Session::new(SR, Layout::Mono).without_limiter()
}

/// A session with the setup already in place and the clock at zero.
pub fn ready() -> Session {
    let mut s = session();
    s.eval(SETUP, Some("now")).unwrap();
    s
}

pub fn run(s: &mut Session, frames: usize) -> Vec<f32> {
    s.process(frames).remove(0)
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}

pub fn crossings(x: &[f32]) -> usize {
    x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
}

pub fn with_tail() -> Session {
    let mut s = session();
    s.eval(REVERB, Some("now")).unwrap();
    s.eval("src(freq: 100hz) for 1/8beat", Some("now")).unwrap();
    s
}

pub fn hz(x: &[f32]) -> f32 {
    crossings(x) as f32 / (x.len() as f32 / SR)
}

pub const KEYBOARD: &str = "
tempo 120bpm
instr organ(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }
clip pad { instrument: organ
  keys: midi.keys(channel: 1) }
pad.start
";

pub const EXPRESSIVE: &str = "
tempo 120bpm
instr organ(freq: hz, bend: st = 0st, pressure: 0..1 = 0) { osc(sine, freq + bend) * (0.2 + pressure * 0.6) * env[1 | 10ms 0] }
clip pad { instrument: organ
  keys: midi.keys(channel: 1) }
pad.start
";

pub const REVERB: &str = "
bus space
instr click(freq: hz) { space += osc(sine, 1000hz) * env[1 0.5ms 0] }
track hall { out = space.reverb(room: 0.8, damp: 0.5) }
track src { instrument = click }
";
