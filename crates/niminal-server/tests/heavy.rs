//! CPU benchmark for the parallel-DSP work: how long heavy pieces take to render.
//! `cargo test --release -p niminal-server --test heavy -- --ignored --nocapture`

mod support;

use niminal_server::{At, Op, Policy, Pool, Session};
use niminal_lang::Layout;
use niminal_score::{Time, Value};
use std::sync::Arc;
use std::time::Instant;
use support::Eval;

const SR: f32 = 48_000.0;
const SECONDS: usize = 2;

const VOICE: &str = "instr {name}(freq: hz, amp: db = -24db) {\n  level = env[0 5ms 1 | 200ms 0]\n  out = (osc(saw, freq) + osc(saw, freq * 1.003) + osc(saw, freq * 0.997) + osc(square, freq * 0.5))\n    .lpf(cutoff: freq * 6, res: 0.3)\n    .gain(amp) * level\n{send}}\n";

fn instrument(name: &str, send: &str) -> String {
    VOICE.replace("{name}", name).replace("{send}", send)
}

fn note(node: &str, hz: f64) -> Op {
    Op::Note {
        target: node.into(),
        to: None,
        at: At::Now,
        args: [("freq".to_string(), Value::Hz(hz))].into(),
        dur: Some(Time::Seconds(10.0)),
        id: None,
        tag: None,
    }
}

/// Render the piece with `workers` helper threads: how long it took, and what it sounded like.
fn render(source: &str, notes: &[(String, usize)], workers: usize, policy: Policy) -> (f64, Vec<Vec<f32>>) {
    let mut s = Session::new(SR, Layout::Stereo).without_limiter();
    if workers > 0 {
        s.set_fork(Some(Arc::new(Pool::new(workers, policy, || {}))));
    }
    s.eval(source, None).unwrap();
    for (node, count) in notes {
        let ops = (0..*count)
            .map(|i| note(node, 110.0 * 1.0595f64.powi(i as i32 % 24)))
            .collect();
        s.send_events(0, ops).unwrap();
    }
    let start = Instant::now();
    let out = s.process(SR as usize * SECONDS);
    (start.elapsed().as_secs_f64(), out)
}

fn time(label: &str, source: &str, notes: &[(String, usize)]) {
    let blocks = (SR as usize * SECONDS) as f64 / niminal_engine::BLOCK as f64;
    let (base, reference) = render(source, notes, 0, Policy::Never);
    print!("{label:<34} serial {:>6.1}", base * 1e6 / blocks);
    for (name, workers, policy) in [
        ("auto+3", 3, Policy::Auto),
        ("always+3", 3, Policy::Always),
        ("never+3", 3, Policy::Never),
        ("auto+7", 7, Policy::Auto),
    ] {
        let (took, out) = render(source, notes, workers, policy);
        assert!(out == reference, "{label}: {name} sounds different");
        print!("  {name} {:>6.1} ({:.1}x)", took * 1e6 / blocks, base / took);
    }
    println!();
}

#[test]
#[ignore]
fn heavy_pieces() {
    // 64 voices on one node
    let pad = format!("{}node pad {{ instrument = voice }}", instrument("voice", ""));
    time("1 node, 256 voices", &pad, &[("pad".into(), 256)]);

    // 8 nodes, 32 voices each
    let mut eight = instrument("voice", "");
    let mut notes = Vec::new();
    for t in 0..8 {
        eight += &format!("processor t{t} {{ instrument = voice }}\n");
        notes.push((format!("t{t}"), 32));
    }
    time("8 nodes, 32 voices each", &eight, &notes);

    // 4 nodes sending to a delay + reverb bus node
    let mut fx = String::from("bus space\n");
    fx += &instrument("voice", "  space += out.gain(-9db)\n");
    fx += "processor room {\n  echoes = space.delay(time: 280ms, feedback: 0.4).lpf(cutoff: 3khz)\n  out = (echoes + space.reverb(room: 0.8, damp: 0.4)).gain(0.8)\n}\n";
    let mut notes = Vec::new();
    for t in 0..4 {
        fx += &format!("processor t{t} {{ instrument = voice }}\n");
        notes.push((format!("t{t}"), 32));
    }
    time("4 nodes + delay/reverb bus", &fx, &notes);

    // plenty of voices but each costs almost nothing: sharing them out should lose
    let mut light = String::from("instr tone(freq: hz) { osc(sine, freq) * env[0 5ms 1 | 200ms 0] * 0.01 }\n");
    let mut notes = Vec::new();
    for t in 0..16 {
        light += &format!("processor t{t} {{ instrument = tone }}\n");
        notes.push((format!("t{t}"), 6));
    }
    time("16 nodes, 6 sine voices each", &light, &notes);
    for (nodes, voices) in [(2, 2), (4, 2), (16, 1)] {
        let mut tiny = String::from("instr tone(freq: hz) { osc(sine, freq) * env[0 5ms 1 | 200ms 0] * 0.01 }\n");
        let mut notes = Vec::new();
        for t in 0..nodes {
            tiny += &format!("processor t{t} {{ instrument = tone }}\n");
            notes.push((format!("t{t}"), voices));
        }
        time(&format!("{nodes} nodes, {voices} sine voice(s) each"), &tiny, &notes);
    }
    println!("pool overhead (with margin): {} ns", Pool::new(3, Policy::Auto, || {}).overhead_ns());

    // many nodes, each with too few voices to be worth sharing out on its own
    let mut ten = instrument("voice", "");
    let mut notes = Vec::new();
    for t in 0..10 {
        ten += &format!("processor t{t} {{ instrument = voice }}\n");
        notes.push((format!("t{t}"), 10));
    }
    time("10 nodes, 10 voices each", &ten, &notes);

    // few voices, heavy effect chains: nothing here has 16 voices on a node, so nothing forks
    let chain = "it.reverb(room: 0.9, damp: 0.3).reverb(room: 0.8, damp: 0.5).delay(time: 130ms, feedback: 0.5).reverb(room: 0.7, damp: 0.2)";
    for nodes in [1, 6] {
        let mut src = instrument("voice", "");
        let mut notes = Vec::new();
        for t in 0..nodes {
            src += &format!("processor t{t} {{ instrument = voice\n  out = {chain} }}\n");
            notes.push((format!("t{t}"), 4));
        }
        time(&format!("{nodes} node(s), 4 voices, heavy chain"), &src, &notes);
    }
}
