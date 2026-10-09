use niminal_cli::render::{RenderOptions, SAMPLE_RATE, render as render_with};
use niminal_lang::{Program, compile};
use niminal_score::Section;

const SAW_LEAD: &str = include_str!("../../../examples/saw_lead.nml");
const PHI: &str = include_str!("../../../examples/phi.nms");
const SR: f64 = SAMPLE_RATE as f64;

fn render(p: &Program, extra: &[niminal_score::Event]) -> Result<Vec<f32>, niminal_cli::render::RenderError> {
    render_with(p, extra, &RenderOptions::default()).map(|r| r.samples)
}

fn program(src: &str) -> Program {
    compile(src).unwrap_or_else(|errs| {
        panic!("{}", errs.iter().map(|d| d.render("test", src)).collect::<String>());
    })
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

const ONE_NOTE: &str = "
tempo 120bpm
instr lead(freq: hz, amp: db = -6db) {
  level = env[0 5ms 1 200ms 0.6 | 300ms 0]
  osc(saw, freq).lpf(cutoff: 2khz, res: 0.2).gain(amp) * level
}
lead(freq: a3) for 1beat
";

#[test]
fn a_note_sounds_for_its_length_then_rings_out_its_release() {
    let out = render(&program(ONE_NOTE), &[]).unwrap();
    // 1 beat at 120bpm is 0.5s; the 300ms release follows.
    let secs = out.len() as f64 / SR;
    assert!((0.79..0.81).contains(&secs), "{secs}");
    assert!(rms(&out[(0.3 * SR) as usize..(0.5 * SR) as usize]) > 0.05);
    // the release is linear, so it is still fading in its last 10ms, and ends at zero
    assert!(rms(&out[out.len() - 480..]) < 0.01);
    assert!(out[out.len() - 1].abs() < 1e-3);
    assert!(out.iter().all(|s| s.is_finite()));
}

#[test]
fn tempo_changes_when_notes_land() {
    let slow = render(&program(&ONE_NOTE.replace("120bpm", "60bpm")), &[]).unwrap();
    let fast = render(&program(ONE_NOTE), &[]).unwrap();
    assert!(slow.len() > fast.len() + (0.4 * SR) as usize);
}

#[test]
fn rendering_is_deterministic() {
    let p = program(SAW_LEAD);
    assert_eq!(render(&p, &[]).unwrap(), render(&p, &[]).unwrap());
}

#[test]
fn notes_are_placed_sample_accurately_and_overlap() {
    let src = "
tempo 120bpm
instr click() { osc(sine, 1000hz) * env[1 0.5ms 0] }
at 250ms click() for 1beat
at 251ms click() for 1beat
";
    let out = render(&program(src), &[]).unwrap();
    let first = (0.250 * SR) as usize;
    assert!(out[..first].iter().all(|s| *s == 0.0), "silent before the first note");
    assert_eq!(out[first], 0.0, "sine starts at zero");
    assert!(out[first + 10] != 0.0);
}

#[test]
fn the_same_music_from_source_or_from_a_score_file() {
    let instruments = "
instr lead(freq: hz, amp: db = -6db) {
  level = env[0 5ms 1 200ms 0.6 | 300ms 0]
  osc(saw, freq).lpf(cutoff: 2khz, res: 0.2).gain(amp) * level
}
";
    let in_source = format!(
        "{instruments}
lead(freq: e5, amp: -12db) for 1/2beat
at 1/2beat lead(freq: g5, amp: -12db) for 1/2beat
at 1beat lead(freq: b5, amp: -12db) for 1beat
"
    );
    let score = PHI.replace("saw_lead", "lead");
    let events = Section::from_json(&score).unwrap().events;

    let a = render(&program(&in_source), &[]).unwrap();
    let b = render(&program(instruments), &events).unwrap();
    assert!(!a.is_empty());
    assert_eq!(a, b);
}

#[test]
fn the_examples_render() {
    let p = program(SAW_LEAD);
    let events = Section::from_json(PHI).unwrap().events;
    let out = render(&p, &events).unwrap();
    assert!(out.len() > (3.0 * SR) as usize);

    let room = render(&program(include_str!("../../../examples/room.nml")), &[]).unwrap();
    assert!(room.len() > (2.0 * SR) as usize);
    assert!(rms(&room) > 0.01);
}

#[test]
fn score_events_are_checked_against_the_instrument() {
    let p = program(ONE_NOTE);
    let render_score = |json: &str| {
        let events = Section::from_json(json).unwrap().events;
        render(&p, &events).unwrap_err().to_string()
    };
    let score = |target: &str, args: &str| {
        format!(
            r#"{{"version": 1, "section": "s", "length": "1beat",
                "events": [{{"at": "0beat", "dur": "1beat", "target": "{target}", "args": {args}}}]}}"#
        )
    };

    assert_eq!(render_score(&score("lede", r#"{"freq": "a3"}"#)), "note for `lede`: no instrument or track named `lede`");
    assert_eq!(
        render_score(&score("lead", r#"{"freq": "440"}"#)),
        "note for `lead`: argument `freq`: expected a frequency (hz or a note name), got `440` (did you mean `440hz`?)"
    );
    assert_eq!(
        render_score(&score("lead", r#"{"freq": "6db"}"#)),
        "note for `lead`: argument `freq`: expected a frequency (hz or a note name), got `6db`"
    );
    assert_eq!(render_score(&score("lead", "{}")), "note for `lead`: `lead` needs an argument `freq`");
    assert_eq!(
        render_score(&score("lead", r#"{"freq": "a3", "amq": "-6db"}"#)),
        "note for `lead`: `lead` has no parameter `amq` (did you mean `amp`?)"
    );
}

#[test]
fn a_program_with_no_notes_renders_nothing() {
    let p = program("instr a() { osc(sine, 100hz) }");
    assert!(render(&p, &[]).unwrap().is_empty());
}

#[test]
fn the_limiter_keeps_loud_output_under_the_ceiling_and_can_be_turned_off() {
    let p = program("instr loud() { osc(saw, 220hz) * 4 }\nloud() for 1beat");
    let limited = render_with(&p, &[], &RenderOptions { limiter: true }).unwrap().samples;
    let raw = render_with(&p, &[], &RenderOptions { limiter: false }).unwrap().samples;
    let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak(&raw) > 3.0);
    assert!(peak(&limited) <= 0.9661, "{}", peak(&limited));
    assert_eq!(limited.len(), raw.len(), "latency is compensated");
}

#[test]
fn the_limiter_does_not_move_notes() {
    let src = "instr click() { osc(sine, 1000hz) * env[1 0.5ms 0] }\nat 250ms click() for 1beat";
    let p = program(src);
    let out = render(&p, &[]).unwrap();
    let first = (0.250 * SR) as usize;
    assert!(out[..first].iter().all(|s| s.abs() < 1e-6), "silent before the note");
    assert!(out[first..first + 200].iter().any(|s| s.abs() > 0.1), "sounds right at the note start");
}

mod opcodes {
    use super::*;

    const LIB: &str = "
opcode one_pole(x, cutoff: hz) {
  state y = 0.0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  y
}

opcode drive(x, amount: 0..1 = 0.5) {
  (x * (1 + amount * 9)).tanh
}

opcode dc_block(x) {
  state x1 = 0.0
  state y1 = 0.0
  y = x - x1 + 0.995 * y1
  x1 = x
  y1 = y
  y
}
";

    fn raw(src: &str) -> Vec<f32> {
        let p = program(&format!("{LIB}{src}"));
        render_with(&p, &[], &RenderOptions { limiter: false }).unwrap().samples
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0, |m, s| m.max(s.abs()))
    }

    #[test]
    fn a_user_lowpass_filters() {
        let bright = raw("instr a() { osc(saw, 2000hz) }\na() for 1beat");
        let dull = raw("instr a() { osc(saw, 2000hz).one_pole(cutoff: 300hz) }\na() for 1beat");
        let mid = bright.len() / 2;
        let (b, d) = (rms(&bright[mid..mid + 9600]), rms(&dull[mid..mid + 9600]));
        assert!(d < b / 4.0, "300hz one-pole should take most of a 2khz saw away: {d} vs {b}");
    }

    #[test]
    fn a_user_waveshaper_bounds_the_signal() {
        let loud = raw("instr a() { osc(sine, 100hz) * 20 }\na() for 1beat");
        let driven = raw("instr a() { (osc(sine, 100hz) * 20).drive(amount: 1) }\na() for 1beat");
        assert!(peak(&loud) > 15.0);
        assert!(peak(&driven) <= 1.0);
        assert!(peak(&driven) > 0.99, "hard-driven tanh nearly reaches 1");
    }

    #[test]
    fn a_user_dc_blocker_removes_offset() {
        let offset = raw("instr a() { osc(sine, 440hz) * 0.3 + 0.5 }\na() for 2beats");
        let blocked = raw("instr a() { (osc(sine, 440hz) * 0.3 + 0.5).dc_block }\na() for 2beats");
        let mean = |x: &[f32]| x.iter().sum::<f32>() / x.len() as f32;
        let tail = |x: &Vec<f32>| x[x.len() / 2..].to_vec();
        assert!((mean(&tail(&offset)) - 0.5).abs() < 0.01);
        assert!(mean(&tail(&blocked)).abs() < 0.02, "{}", mean(&tail(&blocked)));
    }

    #[test]
    fn state_belongs_to_the_voice_so_repeated_notes_sound_identical() {
        let out = raw("instr a() { osc(saw, 300hz).one_pole(cutoff: 500hz) * env[1 0.2sec 1] }\na() for 1/4beat\nat 1beat a() for 1/4beat");
        let second = (0.5 * SR) as usize; // 1 beat at 120bpm
        assert_eq!(&out[..4800], &out[second..second + 4800]);
    }

    #[test]
    fn overlapping_notes_do_not_share_state() {
        let solo = raw("instr a(f: hz) { osc(saw, f).one_pole(cutoff: 500hz) * env[1 1sec 1] }\na(f: 200hz) for 1beat");
        let both = raw("instr a(f: hz) { osc(saw, f).one_pole(cutoff: 500hz) * env[1 1sec 1] }\na(f: 200hz) for 1beat\nat 100ms a(f: 330hz) for 1beat");
        // before the second note starts the output is identical
        assert_eq!(&solo[..4700], &both[..4700]);
        assert_ne!(&solo[5000..6000], &both[5000..6000]);
    }
}

mod routing {
    use super::*;

    fn raw(src: &str) -> Vec<f32> {
        render_with(&program(src), &[], &RenderOptions { limiter: false }).unwrap().samples
    }

    fn close(a: &[f32], b: &[f32]) {
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b).enumerate() {
            assert!((x - y).abs() < 1e-5, "sample {i}: {x} vs {y}");
        }
    }

    #[test]
    fn a_send_reaches_a_track_that_reads_the_bus() {
        let dry = raw("instr tone(f: hz) { osc(sine, f) * env[1 1sec 1] }\ntone(f: 220hz) for 1beat");
        let wet = raw("
bus space
instr tone(f: hz) {
  out = osc(sine, f) * env[1 1sec 1]
  space += out
}
track room { out = space }
tone(f: 220hz) for 1beat");
        // dry plus an identical copy through the bus
        let doubled: Vec<f32> = dry.iter().map(|s| s * 2.0).collect();
        close(&wet, &doubled);
    }

    #[test]
    fn declaration_order_makes_no_difference_and_adds_no_latency() {
        let a = raw("
bus space
instr tone(f: hz) { out = osc(sine, f) * env[1 1sec 1]\nspace += out }
track room { out = space.gain(0.5) }
tone(f: 220hz) for 1beat");
        let b = raw("
track room { out = space.gain(0.5) }
instr tone(f: hz) { out = osc(sine, f) * env[1 1sec 1]\nspace += out }
bus space
tone(f: 220hz) for 1beat");
        close(&a, &b);
        // the reader hears the send in the same block: 1.5 x the dry signal from sample 0
        let dry = raw("instr tone(f: hz) { osc(sine, f) * env[1 1sec 1] }\ntone(f: 220hz) for 1beat");
        let scaled: Vec<f32> = dry.iter().map(|s| s * 1.5).collect();
        close(&a, &scaled);
    }

    #[test]
    fn a_track_chain_processes_its_instrument() {
        let dry = raw("instr tone(f: hz) { osc(sine, f) * env[1 1sec 1] }\ntone(f: 220hz) for 1beat");
        let through = raw("
instr tone(f: hz) { osc(sine, f) * env[1 1sec 1] }
track lead { instrument = tone\nout = it.gain(0.25) }
lead(f: 220hz) for 1beat");
        let quarter: Vec<f32> = dry.iter().map(|s| s * 0.25).collect();
        close(&through, &quarter);
    }

    #[test]
    fn routing_a_track_to_a_bus_moves_its_sound() {
        let direct = raw("
instr tone(f: hz) { osc(sine, f) * env[1 1sec 1] }
track lead { instrument = tone\nout = it.gain(0.5) }
lead(f: 220hz) for 1beat");
        let via_bus = raw("
bus mix
instr tone(f: hz) { osc(sine, f) * env[1 1sec 1] }
track lead { instrument = tone\nout = it.gain(0.5).to(mix) }
track out_track { out = mix }
lead(f: 220hz) for 1beat");
        close(&direct, &via_bus);
    }

    #[test]
    fn track_defaults_apply_and_notes_override_them() {
        let src = |track_arg: &str, note_arg: &str| {
            format!(
                "instr tone(f: hz, amp: db = 0db) {{ osc(sine, f).gain(amp) * env[1 1sec 1] }}
track lead {{ instrument = tone({track_arg}) }}
lead({note_arg}) for 1beat"
            )
        };
        let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let loud = peak(&raw(&src("f: 220hz", "f: 220hz")));
        let quiet_default = peak(&raw(&src("f: 220hz, amp: -12db", "f: 220hz")));
        let overridden = peak(&raw(&src("f: 220hz, amp: -12db", "f: 220hz, amp: 0db")));
        assert!((quiet_default - loud * 0.2512).abs() < 0.01, "{quiet_default} vs {loud}");
        assert!((overridden - loud).abs() < 1e-4);
    }

    #[test]
    fn a_chains_filter_state_persists_across_notes() {
        // One track-level filter serves both notes. When the second note
        // starts, the filter still holds charge from the first, so its output
        // starts well above where the first note's did.
        let src = "
opcode one_pole(x, cutoff: hz) {
  state y = 0.0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  y
}
instr dc(f: hz) { 1.0 * env[1 | 1ms 0] }
track lead { instrument = dc\nout = it.one_pole(cutoff: 2hz) }
lead(f: 1hz) for 1/8beat
at 1/4beat lead(f: 1hz) for 1/8beat
";
        let out = raw(src);
        let second_start = (0.125 * SR) as usize; // 1/4 beat at 120bpm
        assert!(out[1] < 0.01, "the filter starts empty: {}", out[1]);
        assert!(out[second_start + 1] > 0.1, "and still holds charge later: {}", out[second_start + 1]);
    }

    #[test]
    fn feedback_is_rejected_when_compiling() {
        let errs = compile("bus x\nbus y\ntrack a { out = x.to(y) }\ntrack b { out = y.to(x) }").err().unwrap();
        assert!(errs[0].message.starts_with("tracks feed back through buses"));
    }
}
