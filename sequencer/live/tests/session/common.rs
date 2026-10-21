pub use niminal_lang::Layout;
pub use niminal_live::{Offline, QuantizeDefaults};
pub use niminal_score::{Time, Value};
pub use niminal_server::{At, Op, from_lines, to_lines};

pub const SR: f32 = 48_000.0;
pub const BAR: usize = 96_000; // 120bpm in 4/4

pub const SETUP: &str = "\ntempo 120bpm\ninstr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip lead { instrument: pluck }\nclip bass { instrument: pluck }\n";

pub fn session() -> Offline {
    Offline::new(SR, Layout::Mono).without_limiter()
}

/// A session with the setup already in place and the clock at zero.
pub fn ready() -> Offline {
    let mut s = session();
    s.eval(SETUP, Some("now")).unwrap();
    s
}

pub fn run(s: &mut Offline, frames: usize) -> Vec<f32> {
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

pub fn with_tail() -> Offline {
    let mut s = session();
    s.eval(REVERB, Some("now")).unwrap();
    s.eval("src(freq: 100hz) for 1/8beat", Some("now")).unwrap();
    s
}

pub fn hz(x: &[f32]) -> f32 {
    crossings(x) as f32 / (x.len() as f32 / SR)
}

pub const KEYBOARD: &str = "\ntempo 120bpm\ninstr organ(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\nclip pad { instrument: organ\n  keys: midi.keys(channel: 1) }\npad.start\n";

pub const EXPRESSIVE: &str = "\ntempo 120bpm\ninstr organ(freq: hz, bend: st = 0st, pressure: 0..1 = 0) { osc(sine, freq + bend) * (0.2 + pressure * 0.6) * env[1 | 10ms 0] }\nclip pad { instrument: organ\n  keys: midi.keys(channel: 1) }\npad.start\n";

pub const REVERB: &str = "\nbus space\ninstr click(freq: hz) { space += osc(sine, 1000hz) * env[1 0.5ms 0] }\nfx hall { out = space.reverb(room: 0.8, damp: 0.5) }\nclip src { instrument: click }\n";
