pub use niminal_cli::render::{RenderOptions, SAMPLE_RATE, render as render_with};
pub use niminal_lang::{Program, compile};
pub use niminal_score::Section;

pub const SAW_LEAD: &str = include_str!("../../../../examples/saw_lead.nml");

pub const PHI: &str = include_str!("../../../../examples/phi.nms");

pub const SR: f64 = SAMPLE_RATE as f64;

/// The only channel of a mono render.
pub fn mono(out: niminal_cli::render::RenderOutput) -> Vec<f32> {
    assert_eq!(out.channels.len(), 1, "expected a mono render");
    out.channels.into_iter().next().unwrap()
}

pub fn render(p: &Program, extra: &[niminal_score::Event]) -> Result<Vec<f32>, niminal_cli::render::RenderError> {
    render_with(p, extra, &RenderOptions::default()).map(mono)
}

pub fn program(src: &str) -> Program {
    compile(src).unwrap_or_else(|errs| {
        panic!("{}", errs.iter().map(|d| d.render("test", src)).collect::<String>());
    })
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

pub const ONE_NOTE: &str = "
tempo 120bpm
instr lead(freq: hz, amp: db = -6db) {
  level = env[0 5ms 1 200ms 0.6 | 300ms 0]
  osc(saw, freq).lpf(cutoff: 2khz, res: 0.2).gain(amp) * level
}
lead(freq: a3) for 1beat
";

pub const SONG: &str = "
tempo 120bpm
instr tone(freq: hz) { osc(sine, freq) * env[1 | 5ms 0] }
track a { instrument = tone }
track b { instrument = tone }
scene one { a: [c4], b: ~ }
scene two { a: [e4], b: [g4] }
arrangement song { sections: [one.over(2bars) two.over(1bar)] }
";

pub fn arrangement(name: &str) -> RenderOptions {
    RenderOptions { limiter: false, arrangement: Some(name.into()), ..Default::default() }
}

pub const LANE_STAGE: &str = "tempo 120bpm
instr saw1(freq: hz) { osc(saw, freq) * env[1 | 20ms 0] }
track pad(cut: hz = 300hz) { instrument = saw1
  out = it.lpf(cutoff: cut) }
riff = [c5 c5 c5 c5 c5 c5 c5 c5]
scene s { pad: riff }
";

pub fn lane_song(lanes: &str) -> Vec<f32> {
    let src = format!("{LANE_STAGE}arrangement song {{ sections: [s.over(2bars)]\n{lanes} }}");
    mono(render_with(&program(&src), &[], &arrangement("song")).unwrap())
}

pub fn render_command(args: &[&str]) -> (std::process::Output, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("niminal-render-cmd-{}-{}", std::process::id(), args.join("_").len()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("song.nml");
    std::fs::write(&src, SONG).unwrap();
    let out = dir.join("out.wav");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_niminal"))
        .arg("render")
        .arg(&src)
        .args(args)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    (output, out)
}

pub fn seconds_of(wav: &std::path::Path) -> f64 {
    let r = hound::WavReader::open(wav).unwrap();
    f64::from(r.duration()) / f64::from(r.spec().sample_rate)
}
