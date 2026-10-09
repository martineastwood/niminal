//! Mutated programs must never crash or hang the session. Each case takes a
//! real example, damages it in a random way, and sends it to a live session
//! (which compiles it, schedules it and renders a little audio).
//!
//! `NIMINAL_FUZZ_CASES=20000 cargo test --release -p niminal-daemon --test fuzz` runs it longer.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use niminal_daemon::Session;
use niminal_lang::Layout;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const SEEDS: [&str; 6] = [
    include_str!("../../../examples/saw_lead.nml"),
    include_str!("../../../examples/room.nml"),
    include_str!("../../../examples/stereo.nml"),
    include_str!("../../../examples/groove.nml"),
    include_str!("../../../examples/breakbeat.nml"),
    include_str!("../../../examples/song.nml"),
];

const SNIPPETS: [&str; 40] = [
    "[", "]", "{", "}", "(", ")", "~", ".", ",", ":", "=", "+=", "..", "@", "*", "/", "-", "+", "<", ">", "|",
    "0", "1", "-1", "0.0000001", "1e308", "99999999999999999999", "4294967296", "1000000", "0/0",
    "bars", "ms", "hz", "db", "st", "bpm", "launch", "play", "[c4]*100000", "[a b c d e f g h]*64",
];

const WORDS: [&str; 24] = [
    "fast", "slow", "every", "thin", "euclid", "transpose", "shift", "over", "reverse", "rhythm", "reverb", "delay",
    "pan", "lpf", "gain", "env", "osc", "slices", "fit", "choke", "tempo", "meter", "state", "out",
];

fn mutate(rng: &mut Rng, source: &str) -> String {
    let mut text: Vec<char> = source.chars().collect();
    for _ in 0..1 + rng.below(2) {
        if text.is_empty() {
            break;
        }
        let at = rng.below(text.len());
        match [0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 4, 4, 4, 4, 5, 6, 6, 6, 6][rng.below(20)] {
            0 => {
                let len = rng.below(40).min(text.len() - at);
                text.drain(at..at + len);
            }
            1 => {
                let len = rng.below(60).min(text.len() - at);
                let chunk: Vec<char> = text[at..at + len].to_vec();
                let to = rng.below(text.len());
                for (i, c) in chunk.into_iter().enumerate() {
                    text.insert((to + i).min(text.len()), c);
                }
            }
            2 => {
                let s = *rng.pick(&SNIPPETS);
                text.splice(at..at, s.chars());
            }
            3 => {
                let w = *rng.pick(&WORDS);
                text.splice(at..at, format!(" {w} ").chars());
            }
            4 => {
                // replace a number with an extreme one
                if let Some(start) = (at..text.len()).find(|&i| text[i].is_ascii_digit()) {
                    let end = (start..text.len()).find(|&i| !text[i].is_ascii_digit() && text[i] != '.').unwrap_or(text.len());
                    let n = *rng.pick(&["0", "-1", "1e308", "123456789012", "0.000001", "65536", "255"]);
                    text.splice(start..end, n.chars());
                }
            }
            5 => {
                let depth = 1 + rng.below(3000);
                let open = *rng.pick(&['[', '(', '{', '<']);
                let nest: String = std::iter::repeat_n(open, depth).collect();
                text.splice(at..at, nest.chars());
            }
            _ => text[at] = *rng.pick(&['\n', ' ', '"', '\'', '#', '$', '\\', '\u{0}', 'é', '→', '9']),
        }
    }
    text.into_iter().collect()
}

fn cases() -> usize {
    std::env::var("NIMINAL_FUZZ_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(1500)
}

/// Run one source through a fresh session. Returns a description of what went wrong.
fn run(source: &str, layout: Layout, accepted: &mut usize) -> Option<String> {
    let started = Instant::now();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let mut s = Session::new(48_000.0, layout);
        let ok = s.eval(source, Some("now")).is_ok();
        let _ = s.eval("launch verse\nplay lead = [c4 e4 g4]\nhush", Some("now"));
        let _ = s.process(48_000 * 3);
        ok
    }));
    match outcome {
        Err(e) => {
            let why = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_default();
            Some(format!("panicked: {why}"))
        }
        Ok(ok) => {
            *accepted += usize::from(ok);
            (started.elapsed() > Duration::from_secs(10)).then(|| format!("took {:?}", started.elapsed()))
        }
    }
}

#[test]
fn damaged_programs_never_crash_or_hang_a_session() {
    std::panic::set_hook(Box::new(|_| {}));
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut failures: Vec<(String, String)> = Vec::new();
    let mut accepted = 0;
    for case in 0..cases() {
        let seed = rng_seed(&mut rng);
        let source = mutate(&mut rng, seed);
        if let Ok(path) = std::env::var("NIMINAL_FUZZ_TRACE") {
            let _ = std::fs::write(path, &source);
        }
        let layout = if case % 3 == 0 { Layout::Mono } else { Layout::Stereo };
        if let Some(why) = run(&source, layout, &mut accepted)
            && !failures.iter().any(|(w, _)| *w == why)
        {
            failures.push((why, source));
        }
    }
    let _ = std::panic::take_hook();
    eprintln!("{accepted} of {} damaged programs still compiled", cases());
    assert!(accepted * 20 >= cases(), "the damage is too heavy to test anything past the parser");
    if !failures.is_empty() {
        let mut report = String::new();
        for (why, source) in &failures {
            let head: String = source.chars().take(1500).collect();
            report.push_str(&format!("\n=== {why}\n{head}\n"));
        }
        panic!("{} kinds of failure:{report}", failures.len());
    }
}

fn rng_seed(rng: &mut Rng) -> &'static str {
    rng.pick(&SEEDS)
}

const SETUP: &str = "
tempo 120bpm
instr tone(freq: hz) { osc(saw, freq) * env[0 2ms 1 5sec 0] }
track lead { instrument = tone }
";

/// Programs that are legal but ask for far more than any machine can play.
/// None may take long to accept, and the session must keep rendering audio.
const GREEDY: [&str; 10] = [
    "play lead = [c4*1024]",
    "play lead = [[c4*1024]*1024]",
    "play lead = [[[c4*1024]*1024]*1024]",
    "play lead = [c4 e4 g4].fast(100000)",
    "play lead = [c4 e4 g4].fast(1000000000000)",
    "play lead = [c4].euclid(hits: 50000, steps: 100000)",
    "play lead = [c4 e4 g4 b4].repeat_each(100000)",
    "play lead = [c4*64, e4*64, g4*64, b4*64, c5*64, e5*64, g5*64, b5*64]",
    "clip huge { length: 100000bars\n notes: [c4*512] }\nplay lead = huge",
    "play lead = [c4].every(1, it.fast(1000)).fast(1000)",
];

#[test]
fn greedy_programs_are_accepted_quickly_and_playback_carries_on() {
    for src in GREEDY {
        let started = Instant::now();
        let mut s = Session::new(48_000.0, Layout::Stereo);
        s.eval(SETUP, Some("now")).unwrap();
        let accepted = s.eval(src, Some("now")).is_ok();
        let evaluated = started.elapsed();
        let out = s.process(48_000 * 4);
        let total = started.elapsed();
        eprintln!("{src:?}: {} in {evaluated:?}, played in {total:?}", if accepted { "accepted" } else { "rejected" });
        assert!(evaluated < Duration::from_secs(2), "{src}: took {evaluated:?} to accept");
        assert!(total < Duration::from_secs(12), "{src}: took {total:?} to play 4 seconds");
        assert!(out[0].iter().all(|v| v.is_finite()), "{src}");
        assert!(out[0].iter().all(|v| v.abs() <= 1.0), "{src}: the limiter must hold");
    }
}
