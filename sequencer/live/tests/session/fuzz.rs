//! Evaluating again, live, in any order: random runs of definitions sent into a session that is playing,
//! checking after every one what must hold whatever was sent. A failure prints its seed, and the whole
//! run, so that it can be replayed:  `FUZZ_SEED=12 cargo test -p niminal-live --test session fuzz`.
//! `FUZZ_RUNS=500 FUZZ_FROM=100` makes many more runs than the usual few.
//!
//! What must hold:
//! - nothing panics;
//! - an evaluation that is refused changes nothing;
//! - a clip that was running and is not any more is named in a notice;
//! - the project is always one that compiles on its own, and sounds like it: a new session given
//!   only the project's text, with the same clips started, is silent exactly when the live one is
//!   (unless a notice said that a clip keeps its old version because the new one does not work);
//! - the same run twice gives the same project and the same sound.

use crate::common::*;
use std::collections::BTreeSet;

const SETUP: &str = "tempo 120bpm
const mult = 1
const wave = harmonics([1.0], size: 512)
fn double(x) { x * 2 }
ctl cut = 1khz
bus sb
fx fb { out = sb.reverb(room: 0.5, damp: 0.5) }
instr lo(freq: hz = 100hz) { osc(sine, freq) * env[1 | 10ms 0] }
instr hi(freq: hz = 1khz) { osc(sine, freq) * env[1 | 10ms 0] }
k1 = kit { kick: lo\n hat: hi }
pat = [a3 c4]
hits = grid {\n kick: x...\n hat: ..x.\n}
k2 = kit { kick: lo\n clap: hi\n snare: hi\n hat: hi\n open: hi\n perc: lo }
instr i1(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }
instr i2(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }
instr i3(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }
clip c1 { instrument: i1\n notes: pat }
clip c2 { instrument: k1\n notes: hits }
clip c3 { instrument: i2\n notes: [a2] }
scene s1 { c1, c2 }
scene s2 { c3 }
c1.start
c2.start
";

/// A small, fixed generator, so that a seed means the same run everywhere.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

/// One thing a performer might send. Names come from small pools so that they often meet.
fn snippet(r: &mut Rng) -> String {
    let instr = |r: &mut Rng| r.pick(&["i1", "i2", "i3", "lo", "hi"]);
    let clip = |r: &mut Rng| r.pick(&["c1", "c2", "c3", "i1", "lo"]);
    let body = "env[1 | 10ms 0]";
    match r.below(44) {
        0 => format!("instr {}(freq: hz) {{ osc(sine, freq) * {body} }}", instr(r)),
        1 => format!(
            "instr {}(freq: hz, k: 0..1 = 0.5) {{ osc(sine, freq) * (0.5 + k * 0.4) * {body} }}",
            instr(r)
        ),
        2 => format!(
            "instr {}(freq: hz, k: hz) {{ osc(sine, freq + k * 0) * {body} }}",
            instr(r)
        ),
        3 => format!(
            "instr {}(freq: hz) {{ osc(wave, double(freq) * mult) * {body} }}",
            instr(r)
        ),
        4 => format!(
            "instr {}(freq: hz) {{ x = osc(sine, freq) * {body}\n out = x\n sb += x * 0.3 }}",
            instr(r)
        ),
        5 => format!(
            "instr {}(freq: hz) {{ i2 = osc(sine, freq)\n out = i2 * {body} }}",
            instr(r)
        ),
        6 => format!("instr {}(freq: hz = 200hz) {{ osc(sine, freq) * {body} }}", instr(r)),
        7 => format!("const mult = {}", r.pick(&["1", "2"])),
        8 => format!(
            "const wave = harmonics({}, size: 512)",
            r.pick(&["[1.0]", "[0.0, 1.0]", "[1.0, 0.5]"])
        ),
        9 => format!("fn double(x) {{ x * {} }}", r.pick(&["2", "4"])),
        10 => r.pick(&["bus mult", "const sb = 1", "bus sb", "fn pat(x) { x }"]).to_string(),
        11 => format!(
            "clip {} {{ instrument: {}\n notes: [a3 c4] }}",
            clip(r),
            instr(r)
        ),
        12 => format!(
            "clip {} {{ instrument: {}\n notes: [a3 c4]\n k: [0.2 0.8] }}",
            clip(r),
            instr(r)
        ),
        13 => format!("clip {} {{ instrument: {}\n notes: pat }}", clip(r), instr(r)),
        14 => format!("pat = {}", r.pick(&["[a3 ~ c4 ~]", "[a4 a4]", "[a2]"])),
        15 => format!("clip {} {{ instrument: k1\n notes: hits }}", clip(r)),
        16 => format!(
            "hits = grid {{\n {}\n}}",
            r.pick(&["kick: x...", "hat: ..x.", "kick: x...\n hat: ..x.", "kick: x.x."])
        ),
        17 => format!(
            "k1 = kit {{ {} }}",
            r.pick(&[
                "kick: lo\n hat: hi",
                "hat: hi",
                "kick: hi\n hat: lo",
                "kick: lo\n hat: hi\n snare: i1"
            ])
        ),
        18 => format!(
            "clip kb {{ instrument: {}\n keys: midi.keys(channel: 1) }}\nkb.start",
            instr(r)
        ),
        19 => format!("{}.start", clip(r)),
        20 => format!(
            "clip {} {{ instrument: {}\n notes: [a3 c4] }}\n{0}.start",
            clip(r),
            instr(r)
        ),
        21 => format!(
            "instr {}(freq: hz, amp: -40db..6db = -6db) {{ osc(sine, freq) * amp * {body} }}",
            instr(r)
        ),
        22 => format!("{}.stop", clip(r)),
        23 => format!("fx fb {{ out = sb.reverb(room: {}, damp: 0.5) }}", r.pick(&["0.2", "0.8"])),
        24 => format!(
            "clip {} {{ instrument: {}\n notes: [a3 c4]\n gain: [0.5 1] }}",
            clip(r),
            instr(r)
        ),
        26 => format!(
            "clip {} {{ instrument: k2\n notes: drum_grid(theme: {}, density: {}, chaos: {}, seed: {}) }}",
            clip(r),
            r.pick(&["techno", "house", "two_step", "halftime", "breaks", "boom_bap", "trap", "dnb"]),
            r.pick(&["0.3", "0.7", "1"]),
            r.pick(&["0", "0.4"]),
            r.below(5)
        ),
        27 => format!(
            "k2 = kit {{ {} }}",
            r.pick(&[
                "kick: lo\n clap: hi\n snare: hi\n hat: hi\n open: hi\n perc: lo",
                "kick: lo\n hat: hi",
                "kick: hi\n clap: lo\n snare: lo\n hat: lo\n open: lo\n perc: hi",
            ])
        ),
        28 => format!("ctl cut = {}", r.pick(&["1khz", "500hz", "2khz.smooth(10ms)"])),
        29 => format!(
            "instr {}(freq: hz) {{ osc(saw, freq).lpf(cutoff: cut) * {body} }}",
            instr(r)
        ),
        30 => r
            .pick(&[
                "macro mc {\n input: midi.cc(1)\n cut: 200hz..4khz\n}",
                "macro mc {\n input: midi.cc(2)\n cut: 4khz..200hz\n smooth: 20ms\n}",
                "ctl cut2 = midi.cc(21).range(200hz..4khz)",
            ])
            .to_string(),
        31 => format!("scene {} {{ {}, {} }}", r.pick(&["s1", "s2"]), clip(r), clip(r)),
        32 => format!("{}.{}", r.pick(&["s1", "s2"]), r.pick(&["start", "stop", "mute", "unmute"])),
        33 => format!(
            "{}.{}",
            clip(r),
            r.pick(&[
                "reverse",
                "transpose(+5st)",
                "drop(25%)",
                "undo",
                "clear",
                "mute",
                "unmute",
                "solo",
                "unsolo",
                "set(k: 0.5)"
            ])
        ),
        34 => format!(
            "instr {}(freq: hz = 220hz) {{ osc(saw, freq).lpf(cutoff: synced_lfo({}).unipolar.range(300hz..3khz), res: 0.3) * {body} }}",
            instr(r),
            r.pick(&["1/2", "4", "1"])
        ),
        35 => format!("on midi.note({}) {}.start", r.pick(&["36", "38"]), clip(r)),
        36 => format!(
            "clip {} {{ instrument: {}\n notes: [a3 c4]\n cut: [500hz 2khz] }}",
            clip(r),
            instr(r)
        ),
        37 => format!("bus {}", r.pick(&["cut", "s1", "mc", "pat"])),
        38 => format!(
            "fx {} {{ out = sb.delay(time: clock.beat_length * {}, max: 2sec) }}",
            r.pick(&["tf1", "tf2", "fb"]),
            r.pick(&["1/2", "1", "2"])
        ),
        39 => format!(
            "fx {}(length: division = {}) {{ out = sb.delay(time: length, max: 2sec) }}",
            r.pick(&["tf1", "tf2", "fb"]),
            r.pick(&["1/2beat", "3/16beat", "1bar", "0.2sec"])
        ),
        40 => format!(
            "clip {} {{ instrument: {}\n notes: [a3 c4]\n {} {{ length: {} }} }}",
            clip(r),
            instr(r),
            r.pick(&["tf1", "tf2", "fb"]),
            r.pick(&["1/4beat", "1/3sec", "2beats"])
        ),
        41 => format!(
            "instr {}(freq: hz, beats: 0.25..16 = {}) {{ osc(sine, freq) * (0.7 + 0.3 * synced_lfo(beats)) * {body} }}",
            instr(r),
            r.pick(&["1", "4"])
        ),
        42 => format!(
            "fx {}(beats: 0.5..64 = {}) {{ out = sb.phaser(rate: 1hz / (beats * clock.beat_length / 1sec), depth: 0.5, center: 800hz, feedback: 0.3, mix: 0.5) }}",
            r.pick(&["tf1", "tf2", "fb"]),
            r.pick(&["2", "8"])
        ),
        43 => format!(
            "control fn wob(rate: hz) {{ osc(sine, rate) }}\ninstr {}(freq: hz) {{ osc(sine, freq) * (1 + wob({}hz) * 0.2).smooth(5ms) * {body} }}",
            instr(r),
            r.pick(&["0.5", "3"])
        ),
        _ => format!("tempo {}bpm", r.pick(&["120", "90", "140"])),
    }
}

/// Everything about the session that an evaluation could change, as text.
fn state(s: &mut Offline) -> String {
    let clips: Vec<String> = s
        .clips()
        .iter()
        .map(|c| format!("{}:{}:{}:{}", c.name, c.instrument, c.running, c.muted))
        .collect();
    format!("{}\n{:?}", s.project_source(), clips)
}

fn running(s: &mut Offline) -> BTreeSet<String> {
    s.clips()
        .iter()
        .filter(|c| c.running)
        .map(|c| c.name.clone())
        .collect()
}

/// Whether anything sounds: a bar to settle, then one to listen to.
fn sounds(s: &mut Offline) -> bool {
    run(s, BAR);
    rms(&run(s, BAR)) > 0.01
}

/// One run: every step's snippets and outcome are pushed to `log`, then the checks follow.
fn one_run(seed: u64, log: &mut Vec<String>) -> Result<String, String> {
    let mut r = Rng(seed);
    let mut s = session();
    s.eval(SETUP, Some("now")).map_err(|p| format!("setup: {p:?}"))?;
    let mut heard = Vec::new();
    let mut left_out_said = false;
    let mut kept_said = false;
    // mute, solo and live edits change what sounds without changing the project text
    let mut edited = false;
    for step in 0..10 {
        let src = (0..1 + r.below(3))
            .map(|_| snippet(&mut r))
            .collect::<Vec<_>>()
            .join("\n");
        log.push(format!("--- step {step}\n{src}"));
        let before = state(&mut s);
        let was_running = running(&mut s);
        let result = s.eval(&src, Some("now"));
        let notices = s.take_notices();
        left_out_said |= notices.iter().any(|n| n.contains("left out"));
        edited |= ["reverse", "transpose", "drop(", ".mute", ".solo", ".set("]
            .iter()
            .any(|w| src.contains(w));
        kept_said |= notices.iter().any(|n| n.contains("keeps playing as it was"));
        match result {
            Err(p) => {
                log.push(format!("refused: {:?}", p.iter().map(|p| &p.message).collect::<Vec<_>>()));
                let after = state(&mut s);
                if after != before {
                    return Err(format!("a refused evaluation changed the session:\n{before}\n=>\n{after}"));
                }
            }
            Ok(_) => {
                log.push(format!("ok; notices: {notices:?}"));
                // a redefined clip keeps its place and stays running; a new `.stop` ends one on purpose
                let now = running(&mut s);
                for gone in was_running.difference(&now) {
                    let stopped = src.contains(&format!("{gone}.stop"))
                        || ["s1", "s2"].iter().any(|s| src.contains(&format!("{s}.start")) || src.contains(&format!("{s}.stop")));
                    let said = notices.iter().any(|n| n.contains(&format!("`{gone}`")));
                    if !stopped && !said {
                        return Err(format!("clip `{gone}` stopped running without a notice"));
                    }
                }
            }
        }
        // the project on its own, with the same clips started, must sound as the live session does
        let live = sounds(&mut s);
        heard.push(live);
        log.push(format!("running {:?}, sounds {live}, notices after {:?}", running(&mut s), s.take_notices()));
        let mut fresh = session();
        let started: String = running(&mut s)
            .iter()
            .filter(|c| c.as_str() != "kb")
            .map(|c| format!("\n{c}.start"))
            .collect();
        match fresh.eval(&format!("{}{started}", s.project_source()), Some("now")) {
            Ok(_) => {
                // only clips that are running in both can be compared: `started` leaves out the keyboard
                let other = sounds(&mut fresh);
                // a clip that kept its old version, as it said, no longer sounds as the text says
                if live != other && !kept_said && !edited && !running(&mut s).is_empty() {

                    return Err(format!(
                        "live session {} but the project on its own {}\n{}",
                        if live { "sounds" } else { "is silent" },
                        if other { "sounds" } else { "is silent" },
                        s.project_source()
                    ));
                }
            }
            // a drum the kit lost is left out live, but refused in text sent all at once
            Err(_) if left_out_said => {}
            Err(p) => {
                return Err(format!(
                    "the project does not compile on its own: {:?}\n{}",
                    p.iter().map(|p| &p.message).collect::<Vec<_>>(),
                    s.project_source()
                ));
            }
        }
    }
    Ok(format!("{}\n{heard:?}", state(&mut s)))
}

fn run_seed(seed: u64) {
    let mut log = Vec::new();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| one_run(seed, &mut log)));
    let report = |what: &str| {
        panic!("seed {seed}: {what}\n{}", log.join("\n"));
    };
    match outcome {
        Err(_) => report("panicked"),
        Ok(Err(why)) => report(&why),
        Ok(Ok(_)) if !seed.is_multiple_of(5) => {}
        Ok(Ok(first)) => {
            // the same run again must end in the same place, sounding the same
            let again = one_run(seed, &mut Vec::new());
            if again.as_ref() != Ok(&first) {
                report(&format!("a second run differed:\n{first}\n!=\n{again:?}"));
            }
        }
    }
}

#[test]
fn any_order_of_definitions_keeps_the_session_sound() {
    if let Ok(seed) = std::env::var("FUZZ_SEED") {
        return run_seed(seed.parse().unwrap());
    }
    let number = |name: &str, default: u64| {
        std::env::var(name).ok().and_then(|n| n.parse().ok()).unwrap_or(default)
    };
    let from = number("FUZZ_FROM", 0);
    for seed in from..from + number("FUZZ_RUNS", 8) {
        run_seed(seed);
    }
}

/// Found by seed 207: definitions the client took out of the project (a clip took the name of an instrument, and
/// what depended on it went too) were still in the engine, and sounded again when a name they used became a constant.
#[test]
fn a_definition_taken_out_does_not_come_back_in_the_engine() {
    let mut s = session();
    s.eval(SETUP, Some("now")).unwrap();
    let steps = [
        "on midi.note(38) c1.start\ns1.start\nclip i1 { instrument: lo\n notes: [a3 c4] }",
        "fx tf2(beats: 0.5..64 = 8) { out = sb.phaser(rate: 1hz / (beats * clock.beat_length / 1sec), depth: 0.5, center: 800hz, feedback: 0.3, mix: 0.5) }\ninstr i3(freq: hz) { x = osc(sine, freq) * env[1 | 10ms 0]\n out = x\n sb += x * 0.3 }",
        "instr i2(freq: hz, k: hz) { osc(sine, freq + k * 0) * env[1 | 10ms 0] }",
        "bus mult\ninstr lo(freq: hz, k: hz) { osc(sine, freq + k * 0) * env[1 | 10ms 0] }\npat = [a3 ~ c4 ~]",
        "hits = grid {\n kick: x.x.\n}\nclip i1 { instrument: i2\n notes: pat }",
    ];
    for step in steps {
        let _ = s.eval(step, Some("now"));
        run(&mut s, BAR);
    }
    assert!(!s.project_source().contains("fx fb"), "{}", s.project_source());
    s.eval("const sb = 1", Some("now")).unwrap();
    run(&mut s, BAR);
    assert!(rms(&run(&mut s, BAR)) < 0.01, "a definition that was taken out is sounding");
}
