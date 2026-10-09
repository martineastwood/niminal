//! Renders the spec's first example, wired by hand, to `saw_lead.wav`.
//! `cargo run -p niminal-engine --example saw_lead`

use std::sync::Arc;

use niminal_engine::ops::{Env, FilterMode, Gain, Mul, Osc, Svf, Wave};
use niminal_engine::{BLOCK, GraphBuilder, Src, Voice};

const SR: u32 = 48_000;

fn main() {
    let mut g = GraphBuilder::new();
    let freq = g.param("freq", 220.0).unwrap();
    let amp = g.param("amp", 0.5).unwrap();
    let level = g.add(Env::adsr(0.005, 0.2, 0.6, 0.3), &[]).unwrap();
    let osc = g.add(Osc::new(Wave::Saw), &[("freq", freq)]).unwrap();
    let lpf = g
        .add(Svf::new(FilterMode::Low), &[("x", osc), ("cutoff", Src::Const(2000.0)), ("res", Src::Const(0.2))])
        .unwrap();
    let gained = g.add(Gain, &[("x", lpf), ("gain", amp)]).unwrap();
    let out = g.add(Mul, &[("a", gained), ("b", level)]).unwrap();
    let graph = Arc::new(g.build(out));

    let mut writer = hound::WavWriter::create(
        "saw_lead.wav",
        hound::WavSpec { channels: 1, sample_rate: SR, bits_per_sample: 32, sample_format: hound::SampleFormat::Float },
    )
    .unwrap();

    let mut voice = Voice::new(graph, SR as f32);
    let note_off = SR as usize / 2;
    let mut pos = 0;
    let mut buf = [0.0f32; BLOCK];
    while !voice.is_finished() {
        // Split the block at the note-off so the release is sample-accurate.
        let n = if pos < note_off { BLOCK.min(note_off - pos) } else { BLOCK };
        voice.process(&mut buf[..n]);
        buf[..n].iter().for_each(|s| writer.write_sample(*s).unwrap());
        pos += n;
        if pos == note_off {
            voice.release();
        }
    }
    writer.finalize().unwrap();
    println!("wrote saw_lead.wav ({:.2}s)", pos as f32 / SR as f32);
}
