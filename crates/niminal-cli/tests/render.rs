use niminal_cli::render::{RenderOptions, SAMPLE_RATE, render as render_with};
use niminal_lang::{Program, compile};
use niminal_score::Section;

const SAW_LEAD: &str = include_str!("../../../examples/saw_lead.nml");
const PHI: &str = include_str!("../../../examples/phi.nms");
const SR: f64 = SAMPLE_RATE as f64;

/// The only channel of a mono render.
fn mono(out: niminal_cli::render::RenderOutput) -> Vec<f32> {
    assert_eq!(out.channels.len(), 1, "expected a mono render");
    out.channels.into_iter().next().unwrap()
}

fn render(p: &Program, extra: &[niminal_score::Event]) -> Result<Vec<f32>, niminal_cli::render::RenderError> {
    render_with(p, extra, &RenderOptions::default()).map(mono)
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

    let stereo = render_with(
        &program(include_str!("../../../examples/stereo.nml")),
        &[],
        &RenderOptions::default(),
    )
    .unwrap();
    assert_eq!(stereo.channels.len(), 2);
    assert!(stereo.peak() > 0.1 && stereo.peak() <= 0.9661);
    assert!(stereo.len() > (3.0 * SR) as usize);
    assert_ne!(stereo.channels[0], stereo.channels[1]);

    let room = render(&program(include_str!("../../../examples/room.nml")), &[]).unwrap();
    assert!(room.len() > SR as usize);
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
    let limited = mono(render_with(&p, &[], &RenderOptions { limiter: true, until: None }).unwrap());
    let raw = mono(render_with(&p, &[], &RenderOptions { limiter: false, until: None }).unwrap());
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
    assert!(out[first..].iter().any(|s| s.abs() > 0.1), "sounds right at the note start");
    assert!(out.len() < first + 200, "and the silence after the click is trimmed");
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
        mono(render_with(&p, &[], &RenderOptions { limiter: false, until: None }).unwrap())
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
        mono(render_with(&program(src), &[], &RenderOptions { limiter: false, until: None }).unwrap())
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

mod effects {
    use super::*;

    fn raw(src: &str) -> Vec<f32> {
        mono(render_with(&program(src), &[], &RenderOptions { limiter: false, until: None }).unwrap())
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    // A short click sent only to a bus, which a track delays.
    const ECHO: &str = "
tempo 120bpm
bus space
instr click() { space += osc(sine, 1000hz) * env[1 0.5ms 0] }
track fx { out = space.delay(time: 1/4beat, feedback: 0.5) }
click() for 1/8beat
";

    #[test]
    fn echoes_land_on_the_beat_and_decay() {
        let out = raw(ECHO);
        let window = |n: usize| peak(&out[n..n + 200]);
        // 1/4 beat at 120bpm is 6000 samples
        let (first, second, third) = (window(6000), window(12_000), window(18_000));
        assert!(first > 0.5, "{first}");
        assert!((second / first - 0.5).abs() < 0.05, "{second} vs {first}");
        assert!((third / first - 0.25).abs() < 0.05);
        assert!(peak(&out[..6000]) < 1e-6, "silent before the first echo: the instrument itself makes no sound");
        assert!(peak(&out[2000..5900]) < 1e-6);
    }

    #[test]
    fn rendering_waits_for_the_tail_and_trims_the_silence() {
        let out = raw(ECHO);
        // 0.5^n falls below -90db after about 15 echoes of 125ms
        assert!(out.len() > (1.5 * SR) as usize, "{}", out.len() as f64 / SR);
        assert!(out.len() < (3.0 * SR) as usize);
        assert!(out.last().unwrap().abs() >= 3.1e-5, "ends on the last audible sample");
    }

    #[test]
    fn a_reverb_track_rings_after_the_notes_stop() {
        let out = raw("
bus space
instr click() { space += osc(sine, 1000hz) * env[1 0.5ms 0] }
track hall { out = space.reverb(room: 0.8, damp: 0.5) }
click() for 1/8beat");
        assert!(out.len() > SR as usize, "a reverb tail is longer than a second: {}", out.len() as f64 / SR);
        assert!(out.iter().all(|s| s.is_finite()));
        assert!(peak(&out) > 0.001);
    }

    #[test]
    fn a_delay_time_given_as_a_parameter_matches_the_same_literal_time() {
        let with_param = raw("
instr echo(t: sec) { out = (osc(saw, 220hz) * env[1 | 10ms 0]).delay(time: t, max: 1sec, feedback: 0.3) }
echo(t: 100ms) for 1beat");
        let with_literal = raw("
instr echo() { out = (osc(saw, 220hz) * env[1 | 10ms 0]).delay(time: 100ms, feedback: 0.3) }
echo() for 1beat");
        assert!(peak(&with_param) > 0.1);
        assert_eq!(with_param, with_literal);
    }

    #[test]
    fn a_reverb_inside_an_instrument_rings_out_as_it_does_on_a_track() {
        let in_instrument = raw("
instr click() { out = (osc(sine, 1000hz) * env[1 0.5ms 0]).reverb(room: 0.8, damp: 0.5) }
click() for 1/8beat");
        let on_track = raw("
bus space
instr click() { space += osc(sine, 1000hz) * env[1 0.5ms 0] }
track hall { out = space.reverb(room: 0.8, damp: 0.5) }
click() for 1/8beat");
        assert!(in_instrument.len() > SR as usize, "the voice outlived its note: {}", in_instrument.len());
        assert_eq!(in_instrument.len(), on_track.len());
        let worst = in_instrument.iter().zip(&on_track).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(worst < 1e-6, "largest difference {worst}");
    }

    #[test]
    fn a_delay_inside_an_instrument_is_not_cut_off_at_note_end() {
        let out = raw("
tempo 120bpm
instr click() { out = (osc(sine, 1000hz) * env[1 0.5ms 0]).delay(time: 1/2beat, feedback: 0.5) }
click() for 1/16beat");
        // the note ends after 1/16 beat (~31ms); echoes at 250ms intervals still sound
        let at = |n: usize| peak(&out[n..n + 200]);
        assert!(at(12_000) > 0.5, "first echo {}", at(12_000));
        assert!(at(24_000) > 0.2, "second echo {}", at(24_000));
    }

    #[test]
    fn the_example_with_effects_renders() {
        let out = raw(include_str!("../../../examples/room.nml"));
        assert!(out.iter().all(|s| s.is_finite()));
        assert!(rms(&out) > 0.01);
    }
}

mod channels {
    use super::*;

    /// Render with the limiter off and return every channel.
    fn raw(src: &str) -> Vec<Vec<f32>> {
        render_with(&program(src), &[], &RenderOptions { limiter: false, until: None }).unwrap().channels
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    fn crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
    }

    const TONE: &str = "osc(sine, 220hz) * env[1 | 1ms 0]";

    #[test]
    fn a_mono_instrument_is_centred_in_a_stereo_master() {
        let out = raw(&format!("config {{ channels: stereo }}\ninstr a() {{ {TONE} }}\na() for 1beat"));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], out[1]);
        assert!((peak(&out[0]) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "-3db each side: {}", peak(&out[0]));
    }

    #[test]
    fn pan_places_the_sound_between_the_speakers() {
        let at = |az: &str| {
            raw(&format!(
                "config {{ channels: stereo }}\ninstr a() {{ ({TONE}).pan(azimuth: {az}) }}\na() for 1beat"
            ))
        };
        let (left, right) = (at("-30deg"), at("30deg"));
        assert!(peak(&left[0]) > 0.99 && peak(&left[1]) < 1e-6, "hard left");
        assert!(peak(&right[1]) > 0.99 && peak(&right[0]) < 1e-6, "hard right");

        let centre = at("0deg");
        assert_eq!(centre[0], centre[1]);
        let power = |c: &[Vec<f32>], i: usize| c[0][i] * c[0][i] + c[1][i] * c[1][i];
        // constant power across the pan
        let mid = at("12deg");
        let i = 5000;
        assert!((power(&mid, i) - power(&centre, i)).abs() < 1e-4);
        assert!(peak(&mid[1]) > peak(&mid[0]), "12 degrees is right of centre");
    }

    #[test]
    fn surround_pan_uses_the_right_speakers() {
        let at = |az: &str| {
            raw(&format!(
                "config {{ channels: surround(5.1) }}\ninstr a() {{ ({TONE}).pan(azimuth: {az}) }}\na() for 1beat"
            ))
        };
        let ahead = at("0deg");
        assert_eq!(ahead.len(), 6);
        let loud: Vec<usize> = (0..6).filter(|&c| peak(&ahead[c]) > 0.01).collect();
        assert_eq!(loud, [2], "straight ahead is the centre speaker only");

        let behind = at("180deg");
        assert!(peak(&behind[4]) > 0.5 && peak(&behind[5]) > 0.5);
        assert!(peak(&behind[0]) < 1e-6 && peak(&behind[2]) < 1e-6);
        assert_eq!(peak(&behind[3]), 0.0, "the LFE is never panned to");
    }

    #[test]
    fn a_channel_list_puts_each_entry_on_its_own_channel() {
        let out = raw("
config { channels: stereo }
instr a() { [osc(sine, 220hz), osc(sine, 330hz)] * env[1 | 1ms 0] }
a() for 2beats");
        let (l, r) = (&out[0], &out[1]);
        assert!((crossings(l) as f32 - 220.0).abs() <= 2.0, "left is 220hz: {}", crossings(l));
        assert!((crossings(r) as f32 - 330.0).abs() <= 2.0, "right is 330hz: {}", crossings(r));
    }

    #[test]
    fn destructuring_and_rebuilding_can_swap_channels() {
        let plain = raw("
config { channels: stereo }
instr a() { [osc(sine, 220hz), osc(sine, 330hz)] * env[1 | 1ms 0] }
a() for 1beat");
        let swapped = raw("
config { channels: stereo }
instr a() {
  [l, r] = [osc(sine, 220hz), osc(sine, 330hz)] * env[1 | 1ms 0]
  [r, l]
}
a() for 1beat");
        assert_eq!(plain[0], swapped[1]);
        assert_eq!(plain[1], swapped[0]);
    }

    #[test]
    fn a_per_channel_argument_gives_each_channel_its_own_value() {
        let out = raw("
config { channels: stereo }
instr a() { [osc(saw, 110hz), osc(saw, 110hz)].lpf(cutoff: [300hz, 4khz]) * env[1 | 1ms 0] }
a() for 1beat");
        // the brighter filter keeps the saw's sharp edges: more curvature
        let roughness = |c: &[f32]| {
            let d2: Vec<f32> = c.windows(3).map(|w| w[2] - 2.0 * w[1] + w[0]).collect();
            d2.iter().map(|x| x.abs()).sum::<f32>() / peak(c)
        };
        assert!(roughness(&out[1]) > roughness(&out[0]) * 2.0, "{} vs {}", roughness(&out[1]), roughness(&out[0]));
    }

    #[test]
    fn filters_on_stereo_keep_separate_state_per_channel() {
        // the same filter on a stereo pair, fed silence on the right: nothing leaks across
        let out = raw("
config { channels: stereo }
instr a() { [osc(saw, 110hz), 0 * osc(saw, 110hz)].lpf(cutoff: 500hz) * env[1 | 1ms 0] }
a() for 1beat");
        assert!(peak(&out[0]) > 0.1);
        assert_eq!(peak(&out[1]), 0.0);
    }

    #[test]
    fn surround_folds_down_to_stereo_without_the_lfe() {
        // a bus in 5.1 receives mono (centre) and is read by a stereo-master track
        let out = raw("
config { channels: stereo }
bus hall: surround(5.1)
instr a() { hall += osc(sine, 220hz) * env[1 | 1ms 0] }
track t { out = hall }
a() for 1beat");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], out[1]);
        assert!((peak(&out[0]) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "centre folds in at -3db");

        let lfe_only = raw("
config { channels: stereo }
bus hall: surround(5.1)
instr a() { hall += [0, 0, 0, osc(sine, 80hz), 0, 0] }
track t { out = hall }
a() for 1beat");
        assert!(lfe_only.iter().all(|c| peak(c) < 1e-6), "the LFE doesn't reach stereo");
    }

    #[test]
    fn routing_to_a_mono_bus_then_back_up() {
        let out = raw("
config { channels: stereo }
bus m: mono
instr a() { osc(sine, 220hz).pan(azimuth: -30deg) * env[1 | 1ms 0] }
track t { instrument = a\nout = it.to(m) }
track back { out = m }
t() for 1beat");
        // hard-left stereo averaged to mono (half), then centred in stereo at -3db
        assert_eq!(out[0], out[1]);
        let expected = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
        assert!((peak(&out[0]) - expected).abs() < 0.01, "{} vs {expected}", peak(&out[0]));
    }

    #[test]
    fn a_stereo_reverb_decorrelates_the_channels() {
        let out = raw("
config { channels: stereo }
bus space: stereo
instr click() { space += osc(sine, 1000hz) * env[1 0.5ms 0] }
track hall { out = space.reverb(room: 0.8, damp: 0.5) }
click() for 1/8beat");
        assert!(out[0].len() > SR as usize);
        assert_ne!(out[0], out[1]);
        let diff: f32 = out[0].iter().zip(&out[1]).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 0.01);
    }

    #[test]
    fn notes_start_on_the_same_sample_on_every_channel() {
        let out = raw("
config { channels: surround(5.1) }
instr click() { osc(sine, 1000hz).pan(azimuth: 70deg) * env[1 0.5ms 0] }
at 250ms click() for 1beat");
        let first = |c: &[f32]| c.iter().position(|s| s.abs() > 1e-6);
        let starts: Vec<_> = out.iter().map(|c| first(c)).collect();
        // the sine rises from zero, so the first non-zero sample is one after the note start
        let on = starts.iter().flatten().copied().min().unwrap();
        assert!((on as f64 - 0.25 * SR).abs() <= 1.0);
        for s in starts.iter().flatten() {
            assert_eq!(*s, on);
        }
    }

    #[test]
    fn the_limiter_ducks_all_channels_together() {
        let src = "
config { channels: stereo }
instr a() { [osc(sine, 220hz) * 4, osc(sine, 330hz) * 0.2] * env[1 | 1ms 0] }
a() for 1beat";
        let quiet_right = |limiter| {
            let out = render_with(&program(src), &[], &RenderOptions { limiter, until: None }).unwrap().channels;
            let mid = out[1].len() / 2;
            peak(&out[1][mid..mid + 4800])
        };
        let (on, off) = (quiet_right(true), quiet_right(false));
        assert!(on < off * 0.5, "the quiet side is ducked with the loud one: {on} vs {off}");
    }

    #[test]
    fn wav_files_interleave_the_channels() {
        let p = program("config { channels: stereo }\ninstr a() { osc(sine, 220hz).pan(azimuth: 30deg) * env[1 | 1ms 0] }\na() for 1/4beat");
        let out = render_with(&p, &[], &RenderOptions { limiter: false, until: None }).unwrap();
        let dir = std::env::temp_dir().join(format!("niminal-wav-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stereo.wav");
        niminal_cli::render::write_wav(&path, &out.channels).unwrap();

        let mut reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().channels, 2);
        let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
        assert_eq!(samples.len(), out.len() * 2);
        for i in [0, 100, 1000] {
            assert_eq!(samples[2 * i], out.channels[0][i]);
            assert_eq!(samples[2 * i + 1], out.channels[1][i]);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}

mod pitch {
    use super::*;

    fn crossings_per_second(src: &str) -> f32 {
        let p = program(src);
        let out = mono(render_with(&p, &[], &RenderOptions { limiter: false, until: None }).unwrap());
        let n = out.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        n as f32 / (out.len() as f32 / SR as f32)
    }

    #[test]
    fn semitones_move_a_frequency_by_powers_of_two() {
        let hz = crossings_per_second("instr a() { osc(sine, 220hz + 12st) * env[1 | 1ms 0] }\na() for 2beats");
        assert!((hz - 440.0).abs() < 8.0, "an octave up from 220: {hz}");
        let fifth = crossings_per_second("instr a() { osc(sine, 200hz + 7st) * env[1 | 1ms 0] }\na() for 2beats");
        assert!((fifth - 299.7).abs() < 8.0, "a fifth up from 200: {fifth}");
        let down = crossings_per_second("instr a() { osc(sine, 440hz - 12st) * env[1 | 1ms 0] }\na() for 2beats");
        assert!((down - 220.0).abs() < 8.0, "{down}");
    }

    #[test]
    fn a_semitone_parameter_changes_the_pitch_per_note() {
        let src = |up: &str| {
            format!("instr a(up: st = 0st) {{ osc(sine, a3 + up) * env[1 | 1ms 0] }}\na(up: {up}) for 2beats")
        };
        let base = crossings_per_second(&src("0st"));
        let octave = crossings_per_second(&src("12st"));
        assert!((octave / base - 2.0).abs() < 0.05, "{octave} vs {base}");
    }
}

mod performance {
    use super::*;

    const STAGE: &str = "
tempo 120bpm
instr pluck(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq).lpf(cutoff: freq * (2 + bright * 6)) * env[1 | 20ms 0] }
track lead { instrument = pluck }
track bass { instrument = pluck(bright: 0.1) }
";

    fn render_with_bars(body: &str, bars: Option<f64>) -> Result<Vec<f32>, niminal_cli::render::RenderError> {
        let p = program(&format!("{STAGE}{body}"));
        let until = bars.map(|b| b * 2.0); // a 4/4 bar at 120bpm is two seconds
        render_with(&p, &[], &RenderOptions { limiter: false, until }).map(mono)
    }

    fn raw(body: &str, bars: f64) -> Vec<f32> {
        render_with_bars(body, Some(bars)).unwrap()
    }

    #[test]
    fn a_played_pattern_renders_exactly_like_the_same_notes_written_out() {
        let played = raw("play lead = [c4 e4]", 2.0);
        // two bars of two one-second steps: four notes, two beats each
        let written = raw(
            "lead(freq: c4) for 2beats\nat 2beats lead(freq: e4) for 2beats\nat 4beats lead(freq: c4) for 2beats\nat 6beats lead(freq: e4) for 2beats",
            0.0,
        );
        assert!(!played.is_empty());
        assert_eq!(played, written);
    }

    #[test]
    fn lanes_become_note_parameters() {
        let lane = raw("clip c { notes: [c4 e4]\nbright: [0.1 0.9]\ngain: [-6db] }\nplay lead = c", 1.0);
        let written = raw(
            "lead(freq: c4, bright: 0.1, gain: -6db) for 2beats\nat 2beats lead(freq: e4, bright: 0.9, gain: -6db) for 2beats",
            0.0,
        );
        assert_eq!(lane, written);
    }

    #[test]
    fn a_track_applies_its_instrument_defaults_to_played_notes() {
        let played = raw("play bass = [c2]", 1.0);
        let written = raw("bass(freq: c2) for 4beats", 0.0);
        assert_eq!(played, written);
        let explicit = raw("lead(freq: c2, bright: 0.1) for 4beats", 0.0);
        assert_eq!(played, explicit, "the bass track's bright: 0.1 is the same as writing it");
    }

    #[test]
    fn an_open_ended_piece_needs_a_length() {
        let err = render_with_bars("play lead = [c4]", None).unwrap_err();
        assert!(err.0.contains("plays until it is stopped") && err.0.contains("--bars"), "{err}");
        // but one that ends itself doesn't
        let out = render_with_bars("play lead = [c4]\nat 2 bars hush", None).unwrap();
        assert!(out.len() > (3.9 * SR) as usize && out.len() < (4.5 * SR) as usize, "{}", out.len() as f64 / SR);
    }

    #[test]
    fn the_notes_after_the_length_are_not_started_but_ringing_ones_finish() {
        let out = raw("play lead = [c4]", 1.0);
        // one note starts at 0; the one at 2s is past the end of the render
        let sound = |from: f64, to: f64| rms(&out[(from * SR) as usize..(to * SR) as usize]);
        assert!(sound(0.2, 1.5) > 0.05);
        assert!(out.len() < (2.5 * SR) as usize, "{}", out.len() as f64 / SR);
    }

    #[test]
    fn muting_silences_a_track_for_a_while() {
        let out = raw("play lead = [c4]\nat 1 bar mute lead\nat 3 bars unmute lead", 4.0);
        let loud = |s: f64| rms(&out[(s * SR) as usize..((s + 1.0) * SR) as usize]) > 0.05;
        assert!(loud(0.2), "before the mute");
        assert!(!loud(2.4), "muted (the note from 0s has rung out by 2.4s)");
        assert!(!loud(4.4), "still muted");
        assert!(loud(6.2), "the clip kept its place and sounds again");
    }

    #[test]
    fn solo_isolates_a_track() {
        let both = raw("play lead = [c4]\nplay bass = [c2]", 1.0);
        let solo = raw("play lead = [c4]\nplay bass = [c2]\nsolo bass", 1.0);
        let bass_only = raw("play bass = [c2]", 1.0);
        assert!(rms(&both) > rms(&bass_only));
        assert_eq!(solo, bass_only);
    }

    #[test]
    fn panic_cuts_everything_including_effect_tails() {
        let base = "
bus space
instr ping(freq: hz) { space += osc(sine, freq) * env[1 5ms 0] }
track echo { out = space.delay(time: 100ms, feedback: 0.9) }
track src { instrument = ping }
play src = [c4*2]
";
        let ringing = raw(base, 2.0);
        let cut = raw(&format!("{base}at 1 bar panic"), 2.0);
        assert!(ringing.len() > (4.0 * SR) as usize, "the echoes run on: {}", ringing.len() as f64 / SR);
        // panic at 2s: nothing is heard after it (the echo of the last ping is gone)
        assert!(cut.len() < (2.05 * SR) as usize, "{}", cut.len() as f64 / SR);
        assert!(cut.len() > (1.9 * SR) as usize, "sound up to the panic");
    }

    #[test]
    fn scenes_play_in_sequence() {
        let out = raw("
clip a { notes: [c4] }
clip b { notes: [c2] }
scene one { lead: a }
scene two { lead: b }
launch one
at 1 bar launch two", 2.0);
        let written = raw("lead(freq: c4) for 4beats\nat 4beats lead(freq: c2) for 4beats", 0.0);
        assert_eq!(out, written);
    }

    #[test]
    fn the_groove_example_renders_in_stereo() {
        let p = program(include_str!("../../../examples/groove.nml"));
        let out = render_with(&p, &[], &RenderOptions::default()).unwrap();
        assert_eq!(out.channels.len(), 2);
        assert!(out.len() > (20.0 * SR) as usize, "{}", out.len() as f64 / SR);
        assert!(out.peak() > 0.1 && out.peak() <= 0.9661);
        assert_eq!(out.silenced_voices, 0);
        assert_ne!(out.channels[0], out.channels[1]);
    }
}

mod command_line {
    use std::process::Command;

    fn niminal() -> Command {
        Command::new(env!("CARGO_BIN_EXE_niminal"))
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("niminal-cli-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const OPEN_ENDED: &str = "
instr p(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }
track t { instrument = p }
play t = [c4 e4]
";

    #[test]
    fn bars_bound_a_piece_that_plays_until_stopped() {
        let dir = scratch("bars");
        let src = dir.join("open.nml");
        std::fs::write(&src, OPEN_ENDED).unwrap();
        let wav = dir.join("open.wav");

        let refused = niminal().args(["render", src.to_str().unwrap(), "--out", wav.to_str().unwrap()]).output().unwrap();
        assert!(!refused.status.success());
        assert!(String::from_utf8_lossy(&refused.stderr).contains("--bars"));

        let ok = niminal()
            .args(["render", src.to_str().unwrap(), "--out", wav.to_str().unwrap(), "--bars", "2"])
            .output()
            .unwrap();
        assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
        let said = String::from_utf8_lossy(&ok.stdout);
        assert!(said.contains("1 channel"), "{said}");
        let reader = hound::WavReader::open(&wav).unwrap();
        // 2 bars at the default 120bpm is 4 seconds, plus the last note's release
        let seconds = reader.duration() as f64 / 48_000.0;
        assert!((3.9..5.0).contains(&seconds), "{seconds}");

        let seconds_flag = niminal()
            .args(["render", src.to_str().unwrap(), "--out", wav.to_str().unwrap(), "--seconds", "1"])
            .output()
            .unwrap();
        assert!(seconds_flag.status.success());
        assert!(hound::WavReader::open(&wav).unwrap().duration() < 48_000 * 2);

        let both = niminal()
            .args(["render", src.to_str().unwrap(), "--out", wav.to_str().unwrap(), "--bars", "1", "--seconds", "1"])
            .output()
            .unwrap();
        assert!(!both.status.success(), "the two lengths conflict");
        std::fs::remove_dir_all(dir).ok();
    }
}
