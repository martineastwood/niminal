use niminal_daemon::{QuantizeDefaults, Session, from_lines, to_lines};
use niminal_lang::Layout;
use niminal_score::{Event, Time, Value};

const SR: f32 = 48_000.0;
const BAR: usize = 96_000; // 120bpm in 4/4

const SETUP: &str = "
tempo 120bpm
instr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }
track lead { instrument = pluck }
track bass { instrument = pluck }
";

fn session() -> Session {
    Session::new(SR, Layout::Mono).without_limiter()
}

/// A session with the setup already in place and the clock at zero.
fn ready() -> Session {
    let mut s = session();
    s.eval(SETUP, Some("now")).unwrap();
    s
}

fn run(s: &mut Session, frames: usize) -> Vec<f32> {
    s.process(frames).remove(0)
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, s| m.max(s.abs()))
}

fn crossings(x: &[f32]) -> usize {
    x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
}

#[test]
fn code_evaluated_for_now_sounds_immediately() {
    let mut s = ready();
    let accepted = s.eval("lead.play([c4])", Some("now")).unwrap();
    assert_eq!(accepted.lands_at, 0);
    let out = run(&mut s, 24_000);
    assert!(rms(&out[1000..20_000]) > 0.1);
    assert_eq!(s.take_landed().len(), 2, "the setup and the play");
}

#[test]
fn a_change_waits_for_its_boundary_and_lands_on_exactly_that_sample() {
    let mut s = ready();
    run(&mut s, 10_000);
    // starting a clip defaults to the next cycle: the next bar
    let accepted = s.eval("lead.play([c4])", None).unwrap();
    assert_eq!(accepted.lands_at, BAR as u64);
    assert_eq!(accepted.position, (2, 1.0));
    let pending = s.pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].in_samples, BAR as u64 - 10_000);

    let out = run(&mut s, BAR + 5000 - 10_000);
    assert_eq!(peak(&out[..BAR - 10_000]), 0.0, "silent until it lands");
    assert!(peak(&out[BAR - 10_000..BAR - 10_000 + 2000]) > 0.1, "and sounding right at the boundary");
    assert!(s.pending().is_empty());
    let landed = s.take_landed();
    assert_eq!(landed.last().unwrap().at, BAR as u64);
}

#[test]
fn evaluating_exactly_on_a_boundary_with_next_waits_for_the_following_one() {
    let mut s = ready();
    run(&mut s, BAR);
    let accepted = s.eval("lead.mute @ next bar", None).unwrap();
    assert_eq!(accepted.lands_at, 2 * BAR as u64);
    let plain = s.eval("lead.unmute @ bar", None).unwrap();
    assert_eq!(plain.lands_at, BAR as u64, "the newer supersedes.unmute the pending and.mute lands now");
    assert!(s.pending().is_empty());
}

#[test]
fn code_that_does_not_compile_changes_nothing() {
    let mut s = ready();
    let problems = s.eval("riff = [c4]\nlead.play(nope)", None).unwrap_err();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].message, "`nope` is not defined");
    assert_eq!((problems[0].line, problems[0].column), (Some(2), Some(11)), "located in what was sent");
    assert!(problems[0].help.is_none());
    assert!(s.pending().is_empty());
    // and the good half of the snippet did not sneak in
    assert!(s.eval("lead.play(riff)", Some("now")).is_err());
    // nor did anything sound
    assert_eq!(peak(&run(&mut s, 5000)), 0.0);
}

#[test]
fn syntax_errors_are_located_too() {
    let mut s = ready();
    let problems = s.eval("\n\nlead.play([c4)", None).unwrap_err();
    assert_eq!(problems[0].message, "this `[` is never closed");
    assert_eq!(problems[0].line, Some(3));
    assert!(s.eval("hush @ sometime", None).is_err());
    let e = s.eval("hush", Some("whenever")).unwrap_err();
    assert!(e[0].message.contains("isn't a quantum"));
}

#[test]
fn problems_in_earlier_definitions_say_so() {
    let mut s = ready();
    // a new definition that breaks something defined before it
    let problems = s.eval("instr pluck(freq: hz) { osc(sine, nonsense) }", None).unwrap_err();
    assert!(problems[0].line.is_some(), "the broken line is in what was sent: {problems:?}");
    // A clip that is already playing is not part of the definitions, so an
    // instrument change that breaks it is accepted; its notes then say why they
    // can't play.
    let mut s = ready();
    s.eval("lead.play([c4*4])", Some("now")).unwrap();
    s.eval("instr pluck(pitch: hz) { osc(sine, pitch) }", Some("now")).unwrap();
    run(&mut s, BAR);
    let notices = s.take_notices();
    assert!(!notices.is_empty() && notices[0].contains("could not play"), "{notices:?}");
}

#[test]
fn a_transaction_lands_together_at_the_coarsest_boundary() {
    let mut s = ready();
    s.eval("bass.mute", Some("now")).unwrap();
    let accepted = s.eval("bass.unmute @ next beat\nlead.play([c4]) @ next 4 bars", None).unwrap();
    assert_eq!(accepted.lands_at, 4 * BAR as u64, "both wait for the later");
    assert_eq!(s.pending().len(), 1, "one transaction");
    assert_eq!(accepted.changes.len(), 2);
}

#[test]
fn independent_changes_land_at_their_own_boundaries() {
    let mut s = ready();
    let first = s.eval("lead.play([c4]) @ next 4 bars", None).unwrap();
    // This write is independent of the play, so it keeps its nearer boundary.
    let second = s.eval("lead.mute @ next beat", None).unwrap();
    assert!(second.lands_at < first.lands_at);
    run(&mut s, 4 * BAR + 100);
    let ids: Vec<u64> = s.take_landed().iter().map(|l| l.id).collect();
    assert_eq!(ids, [1, 3, 2], "setup, mute, then play");
}

#[test]
fn a_later_evaluation_sees_what_is_still_waiting() {
    let mut s = ready();
    s.eval("riff = [c4]", Some("next 4 bars")).unwrap();
    // `riff` is not defined yet, but it will be by the time this lands
    let ok = s.eval("lead.play(riff)", None);
    assert!(ok.is_ok(), "{ok:?}");
    run(&mut s, 5 * BAR);
    assert!(s.take_notices().is_empty());
}

#[test]
fn cancelling_removes_what_is_waiting() {
    let mut s = ready();
    run(&mut s, 1000); // off the bar line, so these have to wait
    let a = s.eval("lead.play([c4])", None).unwrap();
    s.eval("bass.play([c2])", None).unwrap();
    assert_eq!(s.cancel(a.id), 1);
    assert_eq!(s.pending().len(), 1);
    assert_eq!(s.cancel(None), 1);
    assert!(s.pending().is_empty());
    assert_eq!(peak(&run(&mut s, 2 * BAR)), 0.0, "nothing happened");
    assert_eq!(s.cancel(None), 0);
}

#[test]
fn cancelling_a_change_drops_later_ones_that_depended_on_it() {
    let mut s = ready();
    run(&mut s, 1000);
    let first = s.eval("riff = [c4]", None).unwrap();
    s.eval("lead.play(riff)", None).unwrap(); // uses riff, so it only works with the first
    s.eval("bass.mute", None).unwrap(); // independent
    assert_eq!(s.pending().len(), 3);

    assert_eq!(s.cancel(first.id), 2, "the one cancelled and the one that needed it");
    assert_eq!(s.pending().len(), 1);
    let notices = s.take_notices();
    assert!(notices[0].contains("depended on a change that was cancelled"), "{notices:?}");
    run(&mut s, 2 * BAR);
    assert_eq!(s.take_landed().len(), 2, "the setup and the independent change");
    // and nothing is half-applied
    assert!(s.eval("lead.play(riff)", Some("now")).is_err());
}

#[test]
fn landing_does_no_compiling() {
    // a snippet with a lot to compile is slow to evaluate, and then lands in a
    // fraction of that time: the work is done up front
    let mut s = ready();
    run(&mut s, 1000);
    let big: String = (0..40).map(|i| format!("instr i{i}(freq: hz) {{ osc(saw, freq).lpf(cutoff: 2khz).reverb(room: 0.8) }}\n")).collect();
    let start = std::time::Instant::now();
    let accepted = s.eval(&big, Some("next bar")).unwrap();
    let evaluating = start.elapsed();

    // get close to the landing, then time only the block it happens in
    let lands = accepted.lands_at as usize;
    let to_go = lands - s.clock() as usize;
    run(&mut s, to_go - 64);
    let start = std::time::Instant::now();
    run(&mut s, 128);
    let landing = start.elapsed();
    assert!(s.take_landed().len() >= 2);
    assert!(landing * 5 < evaluating, "evaluating took {evaluating:?} but the block that lands took {landing:?}");
}

#[test]
fn written_notes_play_when_evaluated() {
    let mut s = ready();
    s.eval("lead(freq: a4) for 1beat", None).unwrap();
    let out = run(&mut s, 24_000 + 2000);
    assert!(rms(&out[500..23_000]) > 0.1);
    // 1 beat at 120bpm is 0.5s: the 440hz sine ran for 220 cycles
    assert!((crossings(&out[..24_000]) as i32 - 220).abs() <= 2);
    assert!(peak(&out[24_000 + 1000..]) < 0.05, "and then it stopped");
}

#[test]
fn at_places_a_command_at_an_absolute_time() {
    let mut s = ready();
    s.eval("at bar 3 lead.play([c4])", Some("now")).unwrap();
    let out = run(&mut s, 3 * BAR);
    assert_eq!(peak(&out[..2 * BAR - 1]), 0.0);
    assert!(peak(&out[2 * BAR..2 * BAR + 3000]) > 0.1);
}

#[test]
fn redefining_an_instrument_leaves_sounding_notes_alone_and_changes_new_ones() {
    let a_note = "lead(freq: a3) for 4beats";
    let octave_up = "instr pluck(freq: hz) { osc(sine, freq * 2) * env[1 | 10ms 0] }";

    let mut control = ready();
    control.eval(a_note, Some("now")).unwrap();
    let plain = run(&mut control, BAR);

    let mut live = ready();
    live.eval(a_note, Some("now")).unwrap();
    let mut out = run(&mut live, 20_000);
    live.eval(octave_up, Some("now")).unwrap();
    out.extend(run(&mut live, BAR - 20_000));
    assert_eq!(out, plain, "the note already sounding finishes on the old definition");

    // a note started afterwards uses the new one: an octave up
    live.eval("lead(freq: a3) for 1beat", Some("now")).unwrap();
    let after = run(&mut live, 24_000);
    assert!((crossings(&after[1000..23_000]) as f32 / (22_000.0 / SR) - 440.0).abs() < 12.0, "{}", crossings(&after));
}

#[test]
fn redefining_a_pattern_changes_the_tracks_playing_it() {
    let mut s = ready();
    s.eval("riff = [a3]\nlead.play(riff)", Some("now")).unwrap();
    let before = run(&mut s, 2 * BAR);
    let hz = |x: &[f32]| crossings(x) as f32 / (x.len() as f32 / SR);
    assert!((hz(&before[2000..BAR - 2000]) - 220.0).abs() < 10.0, "playing A3");

    // changing the pattern, not the play line
    s.eval("riff = [a4]", Some("now")).unwrap();
    let after = run(&mut s, 2 * BAR);
    assert!((hz(&after[2000..BAR - 2000]) - 440.0).abs() < 12.0, "now A4, with no new `play`: {}", hz(&after[2000..BAR - 2000]));
}

#[test]
fn changing_the_tempo_retimes_running_clips() {
    let hz = |x: &[f32]| crossings(x) as f32 / (x.len() as f32 / SR);
    let mut s = ready();
    s.eval("lead.play([a3 a4])", Some("now")).unwrap();
    // at 120bpm each of the two steps lasts a second
    let slow = run(&mut s, 2 * 48_000);
    assert!((hz(&slow[2000..46_000]) - 220.0).abs() < 8.0);
    assert!((hz(&slow[50_000..94_000]) - 440.0).abs() < 12.0);

    s.eval("tempo 240bpm", Some("now")).unwrap();
    assert_eq!(s.transport().bpm, 240.0);
    // now each step lasts half a second, and the clip starts over from the change
    let fast = run(&mut s, 3 * 24_000);
    assert!((hz(&fast[1000..23_000]) - 220.0).abs() < 10.0, "first step");
    assert!((hz(&fast[25_000..47_000]) - 440.0).abs() < 14.0, "second step, half a second later");
    assert!((hz(&fast[49_000..71_000]) - 220.0).abs() < 10.0, "and round again after one bar");
}

const REVERB: &str = "
bus space
instr click(freq: hz) { space += osc(sine, 1000hz) * env[1 0.5ms 0] }
track hall { out = space.reverb(room: 0.8, damp: 0.5) }
track src { instrument = click }
";

fn with_tail() -> Session {
    let mut s = session();
    s.eval(REVERB, Some("now")).unwrap();
    s.eval("src(freq: 100hz) for 1/8beat", Some("now")).unwrap();
    s
}

#[test]
fn an_effect_tail_survives_unrelated_changes() {
    let mut control = with_tail();
    let expected = run(&mut control, 2 * BAR);
    assert!(peak(&expected[BAR..]) > 1e-4, "there is a tail to keep");

    let mut live = with_tail();
    let mut out = run(&mut live, 5000);
    live.eval("riff = [c4]\ninstr other(freq: hz) { osc(sine, freq) }", Some("now")).unwrap();
    out.extend(run(&mut live, 2 * BAR - 5000));
    assert_eq!(out, expected, "the reverb's memory carried over the swap");
}

#[test]
fn redefining_a_track_keeps_compatible_effect_memory() {
    let mut live = with_tail();
    run(&mut live, 5000);
    live.eval("track hall { out = space.reverb(room: 0.8, damp: 0.5) * 1.0 }", Some("now")).unwrap();
    let after = run(&mut live, BAR);
    assert!(peak(&after) > 1e-4, "the reverb tail survives the track edit");
}

#[test]
fn adding_a_bus_keeps_sounding_voices() {
    let mut s = ready();
    s.eval("lead(freq: a3) for 4beats", Some("now")).unwrap();
    run(&mut s, 5000);
    s.eval("bus extra", Some("now")).unwrap();
    assert!(peak(&run(&mut s, 5000)) > 0.1);
}

#[test]
fn hush_stops_clips_but_lets_notes_ring_and_panic_stops_everything() {
    let mut s = ready();
    s.eval("lead.play([a3*8])", Some("now")).unwrap();
    run(&mut s, 20_000);
    s.hush().unwrap();
    let after = run(&mut s, BAR);
    // the clip is over: nothing sounds after the last note has finished
    assert!(peak(&after[BAR - 5000..]) == 0.0);

    let mut p = ready();
    p.eval("lead(freq: a3) for 4beats", Some("now")).unwrap();
    run(&mut p, 5000);
    p.panic();
    assert_eq!(peak(&run(&mut p, 1000)), 0.0, "silence at once");
    assert_eq!(p.transport().voices, 0);
}

#[test]
fn mute_and_solo_work_live() {
    let mut s = ready();
    s.eval("lead.play([a3])\nbass.play([a2])", Some("now")).unwrap();
    let both = rms(&run(&mut s, 20_000)[2000..]);
    s.eval("lead.mute", Some("now")).unwrap();
    run(&mut s, BAR);
    let one = rms(&run(&mut s, 20_000)[2000..]);
    assert!(one < both * 0.8, "{one} vs {both}");
    s.eval("lead.unmute\nbass.solo", Some("now")).unwrap();
    run(&mut s, BAR);
    let solo = rms(&run(&mut s, 20_000)[2000..]);
    assert!((solo - one).abs() < 0.02, "only bass again: {solo} vs {one}");
}

#[test]
fn streamed_events_play_at_their_time() {
    let mut s = ready();
    let note = |at: f64| Event {
        target: "lead".into(),
        at: Time::Seconds(at),
        dur: Time::Seconds(0.25),
        args: [("freq".to_string(), Value::Note(57))].into(),
    };
    s.stream_events(vec![note(0.5)]).unwrap();
    let out = run(&mut s, 48_000);
    assert_eq!(peak(&out[..24_000 - 1]), 0.0);
    assert!(peak(&out[24_000..26_000]) > 0.1);
    // a late one sounds at once; a bad one is refused
    s.stream_events(vec![note(0.0)]).unwrap();
    assert!(peak(&run(&mut s, 2000)) > 0.1);
    let bad = Event { target: "nobody".into(), ..note(1.0) };
    assert!(s.stream_events(vec![bad]).is_err());
}

#[test]
fn the_output_layout_is_fixed_for_the_session() {
    let mut s = Session::new(SR, Layout::Stereo);
    assert_eq!(s.channels(), 2);
    let e = s.eval("config { channels: mono }\ninstr a() { 0.5 }", None).unwrap_err();
    assert!(e[0].message.contains("outputs stereo"), "{e:?}");
    // a project that says nothing about channels takes the session's
    s.eval("instr a(freq: hz) { osc(sine, freq) }\ntrack t { instrument = a }\nt.play([c4])", Some("now")).unwrap();
    assert_eq!(s.program().master, Layout::Stereo);
    let out = s.process(2000);
    assert_eq!(out.len(), 2);
    assert!(peak(&out[0]) > 0.01);
}

#[test]
fn the_transport_reports_bars_and_beats() {
    let mut s = ready();
    run(&mut s, BAR + 36_000);
    let t = s.transport();
    assert_eq!((t.bar, t.beat), (2, 2.5));
    assert_eq!(t.sample, (BAR + 36_000) as u64);
    assert!((t.seconds - 2.75).abs() < 1e-9);
    assert_eq!(t.bpm, 120.0);
}

#[test]
fn peaks_and_notices() {
    let mut s = ready();
    s.eval("lead(freq: a3) for 1beat", Some("now")).unwrap();
    run(&mut s, 10_000);
    let peaks = s.take_peaks();
    assert!(peaks[0] > 0.5 && peaks[0] <= 1.0);
    assert_eq!(s.take_peaks()[0], 0.0, "reset after reading");
    assert_eq!(s.take_silenced(), 0);
}

#[test]
fn quantize_defaults_can_be_changed() {
    let mut s = session().with_defaults(QuantizeDefaults {
        play: niminal_lang::analyze("hush @ now").unwrap()[0].quantize.unwrap(),
        ..QuantizeDefaults::default()
    });
    s.eval(SETUP, Some("now")).unwrap();
    let accepted = s.eval("lead.play([c4])", None).unwrap();
    assert_eq!(accepted.lands_at, 0);
}

// ---- determinism ---------------------------------------------------------------

/// Drive a session through a performance, dividing time into blocks of `block`
/// samples and sending each input at the first block boundary at or after the
/// time it was meant for.
fn performance(block: usize) -> Session {
    let inputs: [(usize, &str, Option<&str>); 7] = [
        (0, SETUP, Some("now")),
        (0, "riff = [a3 e4 a4 e4]\nlead.play(riff)", None),
        (30_000, "bass.play([a2*2])", Some("next beat")),
        (70_000, "riff = [a3 c4 e4 g4]", None),
        (150_000, "bass.mute @ next bar", None),
        (210_000, "instr pluck(freq: hz) { osc(saw, freq) * env[1 | 30ms 0] }", Some("next 2 beats")),
        (330_000, "hush", Some("next bar")),
    ];
    let mut s = Session::new(SR, Layout::Mono);
    let mut i = 0;
    while s.clock() < 600_000 {
        while i < inputs.len() && inputs[i].0 as u64 <= s.clock() {
            let (_, src, q) = inputs[i];
            s.eval(src, q).unwrap();
            i += 1;
        }
        s.process(block);
    }
    s
}

#[test]
fn a_replay_reproduces_the_original_audio_exactly() {
    for block in [64, 480, 777, 1024] {
        let mut s = Session::new(SR, Layout::Mono);
        let inputs: [(usize, &str, Option<&str>); 4] = [
            (0, SETUP, Some("now")),
            (0, "riff = [a3 e4 a4 e4]\nlead.play(riff)", None),
            (30_000, "bass.play([a2*2])", Some("next beat")),
            (70_000, "riff = [a3 c4 e4 g4]", None),
        ];
        let mut original: Vec<f32> = Vec::new();
        let mut i = 0;
        while s.clock() < 300_000 {
            while i < inputs.len() && inputs[i].0 as u64 <= s.clock() {
                s.eval(inputs[i].1, inputs[i].2).unwrap();
                i += 1;
            }
            original.extend(s.process(block).remove(0));
        }
        let total = s.clock();
        let replayed = Session::replay(SR, Layout::Mono, QuantizeDefaults::default(), s.log(), total, true)
            .remove(0);
        assert_eq!(original.len(), replayed.len(), "block {block}");
        assert_eq!(original, replayed, "block {block}: replaying the log gives back the audio");
    }
}

#[test]
fn logs_survive_being_written_down() {
    let s = performance(480);
    let text = to_lines(s.log());
    let back = from_lines(&text).unwrap();
    assert_eq!(back, s.log());
    let a = Session::replay(SR, Layout::Mono, QuantizeDefaults::default(), s.log(), 100_000, true);
    let b = Session::replay(SR, Layout::Mono, QuantizeDefaults::default(), &back, 100_000, true);
    assert_eq!(a, b);
}

#[test]
fn a_live_eval_plays_a_kit_from_the_sample_folder_and_picks_up_a_changed_file() {
    let dir = std::env::temp_dir().join(format!("niminal-live-kit-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("kit")).unwrap();
    let write = |name: &str, level: f32| {
        let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(dir.join("kit").join(name), spec).unwrap();
        (0..4800).for_each(|_| w.write_sample((level * 32767.0) as i16).unwrap());
        w.finalize().unwrap();
    };
    write("kick.wav", 0.5);
    write("snare.wav", 0.25);

    let mut s = session();
    s.set_sample_dir(&dir);
    s.eval("tempo 120bpm\nkit k = \"kit\"\ntrack t { instrument = k }", Some("now")).unwrap();
    s.eval("t.play([snare ~ ~ ~])", Some("now")).unwrap();
    let out = run(&mut s, 2000);
    assert!((out[500] - 0.25).abs() < 0.01, "{}", out[500]);

    // code sent from another folder doesn't lose the files the project already named
    let elsewhere = std::env::temp_dir();
    s.set_sample_dir(&elsewhere);
    s.eval("t.play([snare ~ ~ ~])", Some("now")).unwrap();
    s.set_sample_dir(&dir);

    // a bad name is rejected without disturbing what plays
    let problems = s.eval("t.play([snar])", Some("now")).unwrap_err();
    assert!(problems[0].message.contains("did you mean `snare`?") || problems[0].help.as_deref() == Some("did you mean `snare`?"));

    // replacing a file is seen the next time the project is evaluated
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write("snare.wav", -0.25);
    s.eval("kit k = \"kit\"", Some("now")).unwrap();
    s.eval("t.play([~ snare ~ ~])", Some("now")).unwrap();
    let out = run(&mut s, 26_000);
    // the clip starts where it lands, so its second step is a quarter bar later
    assert!(out[24_500] < 0.0, "{}", out[24_500]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compiling_can_happen_away_from_the_session() {
    use niminal_daemon::Begin;
    let mut s = ready();
    let Begin::Job(job) = s.begin_eval("lead.play([c4])", Some("now")) else { panic!("expected something to compile") };
    // the session is free to run while the code compiles elsewhere
    run(&mut s, 1000);
    let done = job.compile();
    let accepted = s.finish_eval(done).unwrap();
    assert_eq!(accepted.lands_at, 1000, "it lands now, which is when the session finished it");
    assert!(rms(&run(&mut s, 12_000)[2000..]) > 0.1);
    // and what the session logged replays to the same sound
    let replayed = Session::replay(SR, Layout::Mono, QuantizeDefaults::default(), s.log(), 13_000, false);
    assert!(rms(&replayed[0][3000..]) > 0.1);
}

#[test]
fn an_evaluation_compiled_before_another_landed_is_compiled_again() {
    use niminal_daemon::Begin;
    let mut s = ready();
    let Begin::Job(first) = s.begin_eval("clip riff { notes: [c4 e4] }", Some("next bar")) else { panic!() };
    let Begin::Job(second) = s.begin_eval("lead.play(riff)", Some("next bar")) else { panic!() };
    // `second` was prepared before `riff` existed; finishing the first moves the project on
    let first = first.compile();
    let second = second.compile();
    s.finish_eval(first).unwrap();
    s.finish_eval(second).expect("it is compiled again against the project that now has `riff`");
    assert_eq!(s.pending().len(), 2);
}

#[test]
fn a_rejected_evaluation_through_the_two_step_path_changes_nothing() {
    use niminal_daemon::Begin;
    let mut s = ready();
    let Begin::Job(job) = s.begin_eval("lead.play([c4 zz])", Some("now")) else { panic!() };
    let problems = s.finish_eval(job.compile()).unwrap_err();
    assert!(problems[0].message.contains("zz"));
    assert!(s.pending().is_empty());
}


#[test]
fn a_new_play_replaces_a_pending_play_for_the_same_track() {
    let mut s = ready();
    run(&mut s, 1000);
    let old = s.eval("lead.play([c4]) @ next 4 bars", None).unwrap();
    let new = s.eval("lead.play([a4]) @ next beat", None).unwrap();
    assert!(new.lands_at < old.lands_at);
    assert_eq!(s.pending().len(), 1);
    assert_eq!(s.pending()[0].id, new.id.unwrap());
    let mut reference = ready();
    run(&mut reference, 1000);
    reference.eval("lead.play([a4]) @ next beat", None).unwrap();
    assert_eq!(run(&mut s, 5 * BAR), run(&mut reference, 5 * BAR));
    let landed = s.take_landed();
    assert!(!landed.iter().any(|l| Some(l.id) == old.id));
}

#[test]
fn replacing_one_write_preserves_the_rest_of_an_older_transaction() {
    let mut s = ready();
    run(&mut s, 1000);
    let old = s.eval("lead.play([c4])
bass.play([c2])", Some("next 4 bars")).unwrap();
    s.eval("lead.play([a4])", Some("now")).unwrap();
    assert_eq!(s.pending().len(), 1);
    assert_eq!(s.pending()[0].id, old.id.unwrap());
    assert_eq!(s.pending()[0].changes, ["bass.play([c2])"]);
    let mut reference = ready();
    run(&mut reference, 1000);
    reference.eval("lead.play([a4])", Some("now")).unwrap();
    reference.eval("bass.play([c2])", Some("next 4 bars")).unwrap();
    assert_eq!(run(&mut s, 5 * BAR), run(&mut reference, 5 * BAR));
}

#[test]
fn a_later_definition_snapshot_cannot_revert_a_newer_write() {
    let mut s = ready();
    run(&mut s, 1000);
    s.eval("instr pluck(freq: hz) { osc(sine, freq) * 0.1 }
clip extra { notes: [c4] }", Some("next 4 bars")).unwrap();
    s.eval("instr pluck(freq: hz) { osc(sine, freq) * 0.8 }", Some("now")).unwrap();
    run(&mut s, 5 * BAR);
    s.eval("pluck(freq: a4) for 1beat", Some("now")).unwrap();
    assert!(peak(&run(&mut s, 5000)) > 0.7);
}

#[test]
fn rejected_replacements_leave_the_pending_transaction_intact() {
    let mut s = ready();
    run(&mut s, 1000);
    let old = s.eval("lead.play([c4])", Some("next bar")).unwrap();
    assert!(s.eval("lead.play(nonexistent)", Some("now")).is_err());
    assert_eq!(s.pending()[0].id, old.id.unwrap());
    run(&mut s, BAR);
    assert!(s.take_landed().iter().any(|l| Some(l.id) == old.id));
}

#[test]
fn an_effect_parameter_edit_keeps_its_tail_and_uses_the_new_setting() {
    let mut live = with_tail();
    run(&mut live, 5000);
    live.eval("track hall { out = space.reverb(room: 0.95, damp: 0.2) }", Some("now")).unwrap();
    assert!(peak(&run(&mut live, BAR)) > 1e-4);
}

#[test]
fn panic_clears_the_limiter_lookahead_as_well_as_the_mixer() {
    let mut s = Session::new(SR, Layout::Mono);
    s.eval(SETUP, Some("now")).unwrap();
    s.eval("pluck(freq: a4) for 4beats", Some("now")).unwrap();
    assert!(peak(&run(&mut s, 1000)) > 0.1);
    let epoch = s.panic_epoch();
    s.panic();
    assert_eq!(epoch.load(std::sync::atomic::Ordering::Acquire), 1);
    assert_eq!(peak(&run(&mut s, 1000)), 0.0);
}


#[test]
fn a_track_edit_keeps_memory_but_applies_the_new_output_gain() {
    let mut live = with_tail();
    let mut reference = with_tail();
    run(&mut live, 5000);
    run(&mut reference, 5000);
    live.eval("track hall { out = space.reverb(room: 0.8, damp: 0.5) * 0.5 }", Some("now")).unwrap();
    let expected = run(&mut reference, BAR);
    let actual = run(&mut live, BAR);
    assert!(peak(&expected) > 1e-4);
    for (a, b) in actual.iter().zip(expected) { assert_eq!(*a, b * 0.5); }
}

#[test]
fn instrument_replacements_do_not_supersede_explicitly_timed_notes() {
    let mut s = ready();
    run(&mut s, 1000);
    s.eval("at bar 3 lead.play([c4])", Some("next bar")).unwrap();
    s.eval("lead.play([a4])", Some("next beat")).unwrap();
    assert_eq!(s.pending().len(), 2);
    run(&mut s, 3 * BAR);
    assert_eq!(s.take_landed().len(), 3);
}

#[test]
fn replacing_a_pending_definition_moves_its_dependent_play_to_the_new_boundary() {
    let mut s = ready();
    run(&mut s, 1000);
    s.eval("riff = [c4]", Some("next beat")).unwrap();
    s.eval("lead.play(riff)", Some("next bar")).unwrap();
    let replacement = s.eval("riff = [a4]", Some("next 4 bars")).unwrap();
    assert_eq!(s.pending().len(), 2);
    assert!(s.pending().iter().all(|p| p.lands_at == replacement.lands_at));
    let mut reference = ready();
    run(&mut reference, 1000);
    reference.eval("riff = [a4]
lead.play(riff)", Some("next 4 bars")).unwrap();
    assert_eq!(run(&mut s, 5 * BAR), run(&mut reference, 5 * BAR));
}


#[test]
fn a_slow_compile_moves_to_the_next_boundary_and_reports_it() {
    let mut s = ready();
    run(&mut s, 23_000);
    let niminal_daemon::Begin::Job(job) = s.begin_eval("lead.play([a4])", Some("next beat")) else { panic!() };
    let done = job.compile();
    run(&mut s, 10_000);
    let accepted = s.finish_eval(done).unwrap();
    assert_eq!(accepted.lands_at, 48_000);
    assert!(s.take_notices().iter().any(|n| n.contains("missed its boundary")));
}

#[test]
fn superseded_pending_changes_replay_to_the_same_audio() {
    let mut s = ready();
    let mut original = run(&mut s, 1000);
    s.eval("riff = [c4]
bass.play([c2])", Some("next 4 bars")).unwrap();
    s.eval("riff = [a4]
lead.play(riff)", Some("now")).unwrap();
    original.extend(run(&mut s, 5 * BAR));
    let replayed = Session::replay(SR, Layout::Mono, QuantizeDefaults::default(), s.log(), original.len() as u64, false);
    assert_eq!(original, replayed[0]);
}

#[test]
fn adding_a_bus_keeps_existing_effect_memory() {
    let mut live = with_tail();
    let mut reference = with_tail();
    run(&mut live, 5000);
    run(&mut reference, 5000);
    live.eval("bus extra", Some("now")).unwrap();
    assert_eq!(run(&mut live, BAR), run(&mut reference, BAR));
}

fn rig_dir(rig: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("niminal-use-{}-{}", std::process::id(), rig.len()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rig.nml"), rig).unwrap();
    dir
}

#[test]
fn used_modules_play_and_replay_without_their_files() {
    let dir = rig_dir("instr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }\ntrack lead { instrument = pluck }\n");
    let mut s = session();
    s.eval_parsed(niminal_daemon::ParsedEval::from_dir("use rig\nlead.play([c4])", &dir, Some("now"))).unwrap();
    let original = run(&mut s, BAR);
    assert!(peak(&original) > 0.1);
    std::fs::remove_dir_all(&dir).unwrap();
    let replayed = Session::replay(SR, Layout::Mono, QuantizeDefaults::default(), s.log(), BAR as u64, false);
    assert_eq!(original, replayed[0]);
}

#[test]
fn a_problem_in_a_used_module_points_at_the_use_line() {
    let dir = rig_dir("instr broken() { nonsense(1) }\n");
    let mut s = session();
    let problems = s.eval_parsed(niminal_daemon::ParsedEval::from_dir("use rig\n", &dir, Some("now"))).unwrap_err();
    assert!(problems[0].message.starts_with("in rig.nml line 1:"), "{}", problems[0].message);
    assert_eq!(problems[0].line, Some(1));
}

#[test]
fn at_plus_counts_from_where_the_code_lands() {
    let mut s = ready();
    run(&mut s, 3 * BAR + 1000);
    s.eval("at +2 bars lead.play([c4])", Some("now")).unwrap();
    let before = run(&mut s, 2 * BAR - 1000);
    let after = run(&mut s, BAR);
    assert!(peak(&before) < 1e-6, "nothing before bar 6");
    assert!(peak(&after) > 0.1, "the note starts two bars after landing");
}

#[test]
fn reset_makes_the_next_bar_line_bar_one() {
    let mut s = ready();
    run(&mut s, 3 * BAR + 1000);
    let accepted = s.eval("reset", None).unwrap();
    assert_eq!(accepted.lands_at, 4 * BAR as u64, "on the next bar line");
    run(&mut s, BAR - 900);
    // bar 5 on the old grid is bar 1 of the new one
    assert_eq!(s.transport().bar, 1);
    let accepted = s.eval("at bar 3 lead.play([c4])", Some("now")).unwrap();
    assert_eq!(accepted.position.0, 1);
    let before = run(&mut s, 2 * BAR - 100);
    let after = run(&mut s, BAR);
    assert!(peak(&before) < 1e-6 && peak(&after) > 0.1, "bar 3 is two bars after the reset, not 3 bars after the start");
}

fn hz(x: &[f32]) -> f32 {
    crossings(x) as f32 / (x.len() as f32 / SR)
}

#[test]
fn a_track_playing_a_launched_scene_can_be_changed_by_name() {
    let mut s = ready();
    s.eval("riff = [c4 e4 g4 b4]\nscene verse { lead: riff, bass: riff }\nverse.start", Some("now")).unwrap();
    let first = run(&mut s, BAR);
    assert!((hz(&first[2000..22_000]) - 262.0).abs() < 10.0, "starts on c4");
    s.eval("lead.reverse", Some("now")).unwrap();
    let second = run(&mut s, BAR);
    assert!((hz(&second[2000..22_000]) - 494.0).abs() < 15.0, "now starts on b4: {}", hz(&second[2000..22_000]));
    // and again: changes build on each other
    s.eval("lead.reverse", Some("now")).unwrap();
    let third = run(&mut s, BAR);
    assert!((hz(&third[2000..22_000]) - 262.0).abs() < 10.0, "back to c4");
}

#[test]
fn a_clip_that_a_scene_plays_follows_a_redefinition() {
    let mut s = ready();
    s.eval("riff = [a3]\nscene verse { lead: riff }\nverse.start", Some("now")).unwrap();
    run(&mut s, BAR);
    s.eval("riff = [a4]", Some("now")).unwrap();
    let after = run(&mut s, BAR);
    assert!((hz(&after[2000..BAR - 2000]) - 440.0).abs() < 12.0, "{}", hz(&after[2000..BAR - 2000]));
}

#[test]
fn stopping_a_scene_stops_its_tracks_and_a_track_with_nothing_playing_cannot_be_changed() {
    let mut s = ready();
    s.eval("riff = [c4]\nscene verse { lead: riff, bass: riff }\nverse.start", Some("now")).unwrap();
    run(&mut s, BAR);
    s.eval("verse.stop", Some("now")).unwrap();
    run(&mut s, 24_000);
    assert!(peak(&run(&mut s, BAR)[10_000..]) < 1e-3, "silent once the notes have rung out");
    let problems = s.eval("lead.reverse", Some("now")).unwrap_err();
    assert!(problems[0].message.contains("isn't playing anything"), "{}", problems[0].message);
}

#[test]
fn changes_stack_and_can_be_undone_or_cleared() {
    let first_beat = |s: &mut Session| hz(&run(s, BAR)[2000..22_000]);
    let mut s = ready();
    s.eval("riff = [c4 e4 g4 b4]\nscene verse { lead: riff }\nverse.start", Some("now")).unwrap();
    assert!((first_beat(&mut s) - 262.0).abs() < 10.0);

    s.eval("lead.reverse", Some("now")).unwrap();
    assert!((first_beat(&mut s) - 494.0).abs() < 15.0, "reversed");
    s.eval("lead.transpose(+12st)", Some("now")).unwrap();
    assert!((first_beat(&mut s) - 988.0).abs() < 30.0, "reversed, then up an octave");

    s.eval("lead.undo", Some("now")).unwrap();
    assert!((first_beat(&mut s) - 494.0).abs() < 15.0, "the octave is off, the reverse stays");
    s.eval("lead.transpose(+12st)\nlead.clear", Some("now")).unwrap();
    assert!((first_beat(&mut s) - 262.0).abs() < 10.0, "everything off, in one go");

    let problems = s.eval("lead.undo", Some("now")).unwrap_err();
    assert!(problems[0].message.contains("nothing to undo"), "{}", problems[0].message);
    s.eval("lead.reverse", Some("now")).unwrap();
    s.eval("verse.start", Some("now")).unwrap();
    assert!((first_beat(&mut s) - 262.0).abs() < 10.0, "launching the scene again starts from its clips");
}

#[test]
fn a_redefined_clip_keeps_the_changes_made_to_a_track() {
    let mut s = ready();
    s.eval("riff = [c4 e4 g4 b4]\nscene verse { lead: riff }\nverse.start\nlead.reverse", Some("now")).unwrap();
    run(&mut s, BAR);
    s.eval("riff = [a3 a4 a4 a4]", Some("now")).unwrap();
    let out = run(&mut s, BAR);
    assert!((hz(&out[2000..22_000]) - 440.0).abs() < 15.0, "reversed: a4 comes first, a3 last: {}", hz(&out[2000..22_000]));
}

#[test]
fn thin_drops_the_same_notes_wherever_it_is_written() {
    let thinned = |code: &str| {
        let mut s = ready();
        s.eval(code, Some("now")).unwrap();
        run(&mut s, 4 * BAR)
    };
    let a = thinned("lead.play([c4 e4 g4 b4 c5 e5 g5 b5].thin(50%))");
    let b = thinned("riff = [c3]\nlead.play([c4 e4 g4 b4 c5 e5 g5 b5].thin(50%))");
    assert_eq!(a, b);
}

#[test]
fn changes_work_on_a_project_that_has_timed_commands_in_it() {
    let mut s = ready();
    s.eval("riff = [c4 e4 g4 b4]\nscene verse { lead: riff }\nscene chorus { lead: riff }\nverse.start\nat +4 bars chorus.start\nat +8 bars hush", Some("now")).unwrap();
    run(&mut s, BAR);
    s.eval("lead.reverse", Some("now")).unwrap();
    assert!((hz(&run(&mut s, BAR)[2000..22_000]) - 494.0).abs() < 15.0, "the later commands didn't change what is playing now");
    s.eval("lead.clear", Some("now")).unwrap();
}

const PARTS: &str = "
tempo 120bpm
instr pluck(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }
clip lead { instrument: pluck
  notes: [c4 e4 g4 b4] }
clip bass { instrument: pluck
  notes: [c2] }
scene verse { lead, bass }
scene bridge { lead }
";

#[test]
fn clips_with_instruments_are_started_changed_muted_and_stopped_by_name() {
    let mut s = session();
    s.eval(PARTS, Some("now")).unwrap();
    s.eval("verse.start", Some("now")).unwrap();
    let on = run(&mut s, BAR);
    assert!(peak(&on) > 0.1, "the scene plays");

    s.eval("lead.reverse", Some("now")).unwrap();
    s.eval("bass.mute", Some("now")).unwrap();
    s.eval("lead.mute", Some("now")).unwrap();
    assert!(peak(&run(&mut s, BAR)[2000..]) < 1e-3, "both muted");
    s.eval("lead.unmute", Some("now")).unwrap();
    let out = run(&mut s, BAR);
    assert!(peak(&out) > 0.1, "lead is back, reversed");
    s.eval("lead.undo", Some("now")).unwrap();
    s.eval("verse.stop", Some("now")).unwrap();
    run(&mut s, 24_000);
    assert!(peak(&run(&mut s, BAR)[10_000..]) < 1e-3, "the scene's parts have stopped");

    s.eval("lead.start", Some("now")).unwrap();
    let again = run(&mut s, BAR);
    assert!(peak(&again) > 0.1 && (hz(&again[2000..22_000]) - 262.0).abs() < 12.0, "one clip started alone, without the reverse: {}", hz(&again[2000..22_000]));
}

#[test]
fn launching_a_scene_replaces_the_parts_it_does_not_list() {
    let mut s = session();
    s.eval(PARTS, Some("now")).unwrap();
    s.eval("verse.start", Some("now")).unwrap();
    run(&mut s, BAR);
    s.eval("bridge.start", Some("now")).unwrap();
    run(&mut s, 24_000);
    let status = s.tracks();
    let names: Vec<_> = status.iter().map(|t| (t.name.as_str(), t.clip.is_some())).collect();
    assert_eq!(names, [("lead", true), ("bass", false)]);
    let problems = s.eval("bass.reverse", Some("now")).unwrap_err();
    assert!(problems[0].message.contains("isn't playing"), "{}", problems[0].message);
}

#[test]
fn a_bus_with_a_body_processes_what_instruments_send_to_it() {
    let heard = |gain: &str| {
        let mut s = Session::new(SR, Layout::Stereo).without_limiter();
        let code = format!("tempo 120bpm\nbus space: stereo {{ out = it.gain({gain}) }}\ninstr ping(freq: hz) {{ out = osc(sine, freq) * env[1 | 10ms 0]\n space += out }}\nclip p {{ instrument: ping\n notes: [c4] }}\np.start");
        s.eval(&code, Some("now")).unwrap();
        let out = s.process(BAR);
        assert!(s.tracks().iter().all(|t| t.name != "space.fx"), "the bus's processing is not a part to play");
        peak(&out[0])
    };
    // the instrument also plays directly; what the bus adds is the rest
    let direct = heard("0");
    let (loud, quiet) = (heard("0.5") - direct, heard("0.25") - direct);
    assert!(quiet > 0.05 && (loud / quiet - 2.0).abs() < 0.1, "the bus's gain shapes the sound: {loud} vs {quiet}");
}

#[test]
fn set_changes_a_lane_live_and_undo_puts_it_back() {
    // the first bit of each bar is the last bar's note dying away
    let level = |s: &mut Session| peak(&run(s, BAR)[2000..]);
    let mut s = session();
    s.eval("tempo 120bpm\ninstr loud(freq: hz, amp: 0..1 = 1) { osc(sine, freq) * amp * env[1 | 10ms 0] }\nclip lead { instrument: loud\n notes: [c4] }\nlead.start", Some("now")).unwrap();
    let full = level(&mut s);
    assert!(full > 0.9, "{full}");
    s.eval("lead.set(amp: 0.25)", Some("now")).unwrap();
    let quiet = level(&mut s);
    assert!((quiet - 0.25).abs() < 0.03, "{quiet}");
    assert_eq!(s.tracks()[0].edits, ["set(amp: 0.25)"]);
    s.eval("lead.set(amp: 0.5)", Some("now")).unwrap();
    assert!((level(&mut s) - 0.5).abs() < 0.03, "the newer set wins");
    s.eval("lead.undo\nlead.undo", Some("now")).unwrap();
    assert!(level(&mut s) > 0.9, "back to the clip as written");
    let problems = s.eval("lead.set(wobble: 1)", Some("now")).unwrap_err();
    assert!(problems[0].message.contains("wobble"), "{}", problems[0].message);
}

#[test]
fn undo_can_name_the_change_to_take_off() {
    let mut s = ready();
    s.eval("riff = [c4 e4 g4 b4]\nscene verse { lead: riff }\nverse.start", Some("now")).unwrap();
    s.eval("lead.reverse\nlead.transpose(+12st)\nlead.thin(10%)", Some("now")).unwrap();
    assert_eq!(s.tracks()[0].edits, ["reverse", "transpose(+12st)", "thin(10%)"]);
    s.eval("lead.undo(reverse)", Some("now")).unwrap();
    assert_eq!(s.tracks()[0].edits, ["transpose(+12st)", "thin(10%)"], "the middle one went, order kept");
    let problems = s.eval("lead.undo(reverse)", Some("now")).unwrap_err();
    assert!(problems[0].message.contains("no `reverse` change to undo"), "{}", problems[0].message);
}

#[test]
fn a_change_named_for_a_clip_goes_to_every_part_playing_it() {
    let mut s = ready();
    s.eval("answer = [c4 e4 g4 b4]\nother = [c3]\nscene chorus { lead: answer, bass: answer.fast(2) }\nchorus.start", Some("now")).unwrap();
    s.eval("answer.reverse", Some("now")).unwrap();
    let edits: Vec<_> = s.tracks().iter().map(|t| t.edits.clone()).collect();
    assert_eq!(edits, [vec!["reverse"], vec!["reverse"]], "both parts play `answer`");
    s.eval("answer.clear", Some("now")).unwrap();
    assert!(s.tracks().iter().all(|t| t.edits.is_empty()));
    let problems = s.eval("other.reverse", Some("now")).unwrap_err();
    assert!(problems[0].message.contains("isn't playing anything"), "{}", problems[0].message);
}

const KEYBOARD: &str = "
tempo 120bpm
instr organ(freq: hz) { osc(sine, freq) * env[1 | 10ms 0] }
clip pad { instrument: organ
  keys: midi.keys(channel: 1) }
pad.start
";

#[test]
fn a_keyboard_clip_plays_the_notes_held_and_lets_go_when_the_key_comes_up() {
    let mut s = session();
    s.eval(KEYBOARD, Some("now")).unwrap();
    assert_eq!(peak(&run(&mut s, 2000)), 0.0, "nothing sounds until a key is pressed");
    assert_eq!(s.key_input(2, 69, 100), 0, "another channel");
    assert_eq!(s.key_input(1, 69, 127), 1);
    let held = run(&mut s, 3 * BAR);
    assert!((hz(&held[2000..BAR]) - 440.0).abs() < 12.0, "a4: {}", hz(&held[2000..BAR]));
    assert!(peak(&held[2 * BAR..]) > 0.5, "still held after three bars");
    assert_eq!(s.key_input(1, 69, 0), 1);
    run(&mut s, 2000);
    assert!(peak(&run(&mut s, BAR)) < 1e-3, "released");
}

#[test]
fn harder_keys_are_louder_and_two_keys_sound_together() {
    let level = |velocity: u8| {
        let mut s = session();
        s.eval(KEYBOARD, Some("now")).unwrap();
        s.key_input(1, 60, velocity);
        peak(&run(&mut s, 4800))
    };
    assert!(level(127) > 2.0 * level(40), "{} vs {}", level(127), level(40));
    let mut s = session();
    s.eval(KEYBOARD, Some("now")).unwrap();
    s.key_input(1, 60, 100);
    s.key_input(1, 67, 100);
    let both = run(&mut s, 9600);
    assert!(peak(&both[2000..]) > 0.35, "two notes add up");
    s.key_input(1, 60, 0);
    run(&mut s, 2400);
    let one = run(&mut s, 4800);
    assert!(peak(&one) > 0.1 && peak(&one) < peak(&both), "the other key still sounds");
}

#[test]
fn a_keyboard_set_replays_from_the_log() {
    let mut s = session();
    s.eval(KEYBOARD, Some("now")).unwrap();
    let mut original = run(&mut s, 1000);
    s.key_input(1, 64, 100);
    original.extend(run(&mut s, 5000));
    s.key_input(1, 64, 0);
    original.extend(run(&mut s, 5000));
    let log = from_lines(&to_lines(s.log())).unwrap();
    let replayed = Session::replay(SR, Layout::Mono, QuantizeDefaults::default(), &log, original.len() as u64, false);
    assert!(peak(&original) > 0.1);
    assert_eq!(original, replayed[0]);
}

#[test]
fn a_keyboard_clip_checks_its_lanes() {
    let mut s = session();
    let both = "tempo 120bpm\ninstr organ(freq: hz) { osc(sine, freq) }\nclip pad { instrument: organ\n keys: midi.keys()\n notes: [c4] }\npad.start";
    let problems = s.eval(both, Some("now")).unwrap_err();
    assert!(problems[0].message.contains("both `notes` and `keys`"), "{}", problems[0].message);
    let bad = "tempo 120bpm\ninstr organ(freq: hz) { osc(sine, freq) }\nclip pad { instrument: organ\n keys: midi.notes() }";
    assert!(s.eval(bad, Some("now")).unwrap_err()[0].message.contains("keys: midi.keys"));
}

#[test]
fn a_mono_instrument_sounds_one_note_at_a_time_and_max_caps_a_poly_one() {
    let sounding = |defs: &str, notes: &str| {
        let mut s = session();
        s.eval(&format!("tempo 120bpm\n{defs}\ntrack t {{ instrument = x }}\n{notes}"), Some("now")).unwrap();
        run(&mut s, 2000);
        s.transport().voices
    };
    let held = "x(freq: 220hz) for 4beats\nx(freq: 330hz) for 4beats\nx(freq: 440hz) for 4beats";
    assert_eq!(sounding("instr x(freq: hz) { osc(sine, freq) }", held), 3, "poly: all three");
    assert_eq!(sounding("mono x(freq: hz) { osc(sine, freq) }", held), 1, "mono: the newest only (the others are fading out)");
    assert_eq!(sounding("instr x(freq: hz) with(max: 2) { osc(sine, freq) }", held), 2);
    assert_eq!(
        sounding("instr x(freq: hz) with(same_note: release) { osc(sine, freq) * env[1 | 5ms 0] }", "x(freq: 220hz) for 4beats\nx(freq: 220hz) for 4beats\nx(freq: 330hz) for 4beats"),
        2,
        "the second 220hz released the first, which has ended by now"
    );
}

#[test]
fn voice_options_are_checked() {
    let mut s = session();
    let err = |code: &str, s: &mut Session| s.eval(code, Some("now")).unwrap_err()[0].message.clone();
    assert!(err("mono x(freq: hz) with(max: 4) { osc(sine, freq) }", &mut s).contains("one voice"));
    assert!(err("instr x(freq: hz) with(legato: true) { osc(sine, freq) }", &mut s).contains("is for `mono`"));
    assert!(err("instr x(freq: hz) with(steal: newest) { osc(sine, freq) }", &mut s).contains("`oldest` or `quietest`"));
    assert!(err("instr x(freq: hz) with(maxx: 3) { osc(sine, freq) }", &mut s).contains("no option `maxx`"));
    assert!(err("instr x(freq: hz) with(max: 0) { osc(sine, freq) }", &mut s).contains("whole number"));
}

#[test]
fn a_new_tempo_keeps_the_clip_in_place_in_its_bar() {
    let mut s = ready();
    s.eval("lead.play([c4 e4 g4 b4])", Some("now")).unwrap();
    // at 120bpm a note lasts half a second: 0.75s in, the second note is half done
    run(&mut s, 36_000);
    s.eval("tempo 240bpm", Some("now")).unwrap();
    assert_eq!(s.transport().bpm, 240.0);
    let after = run(&mut s, 18_000);
    // the bar is now a second long and was 3/8 through: the third note starts an eighth of a bar later
    assert!((hz(&after[7_000..11_000]) - 392.0).abs() < 15.0, "g4 comes next, not c4 again: {}", hz(&after[7_000..11_000]));
}

#[test]
fn a_new_meter_keeps_the_clip_in_place_in_its_bar_and_the_bar_count_going() {
    let mut s = ready();
    s.eval("lead.play([c4 e4 g4 b4])", Some("now")).unwrap();
    // at 120bpm a bar is 2s: 0.75s in, the second note is half done
    run(&mut s, 36_000);
    s.eval("meter 2/4", Some("now")).unwrap();
    let t = s.transport();
    assert_eq!((t.bar, (t.beat * 1000.0).round()), (1, 1750.0), "3/8 through a bar of two beats: 0.75 beats in {t:?}");
    let after = run(&mut s, 18_000);
    // the bar is now a second long and was 3/8 through: the third note starts an eighth of a bar later
    assert!((hz(&after[1_000..5_000]) - 330.0).abs() < 15.0, "still e4: {}", hz(&after[1_000..5_000]));
    assert!((hz(&after[7_000..11_000]) - 392.0).abs() < 15.0, "g4 comes next, not c4 again: {}", hz(&after[7_000..11_000]));
}

#[test]
fn a_meter_change_late_in_a_piece_keeps_the_bar_number_and_bar_lines_in_step_with_the_clips() {
    let mut s = ready();
    s.eval("lead.play([c4 e4 g4 b4])", Some("now")).unwrap();
    // three and a half bars of 4/4
    run(&mut s, 7 * BAR / 2);
    assert_eq!(s.transport().bar, 4);
    s.eval("meter 3/4", Some("now")).unwrap();
    let t = s.transport();
    assert_eq!(t.bar, 4, "{t:?}");
    assert!((t.beat - 2.5).abs() < 0.01, "halfway through the bar, which is now 3 beats: {t:?}");
    // the next bar line is half a 3/4 bar away, and the clip starts over there too
    s.eval("lead.mute", Some("next bar")).unwrap();
    let all = run(&mut s, 40_000);
    assert!(rms(&all[30_000..35_900]) > 0.5);
    assert!(rms(&all[38_000..]) < 1e-6, "muted at the bar line, half a 3/4 bar on, once its release is over: {}", rms(&all[38_000..]));
}

#[test]
fn a_tempo_change_can_name_when_it_lands_and_ramp_over_a_stretch() {
    let mut s = ready();
    s.eval("tempo 120bpm @ next bar", None).unwrap();
    assert_eq!(s.pending().len(), 1);
    let accepted = s.eval("tempo 180bpm @ now over 1 bar", Some("now")).unwrap();
    assert!(s.pending().len() >= 2, "a ramp is a run of changes: {}", s.pending().len());
    assert_eq!(accepted.changes, ["tempo 123.750bpm"], "the first step: 16 steps from 120 to 180");
    let mut seen = vec![s.transport().bpm];
    for _ in 0..40 {
        run(&mut s, 6_000);
        seen.push(s.transport().bpm);
    }
    assert!(seen.windows(2).all(|w| w[1] >= w[0]), "it only speeds up: {seen:?}");
    assert!(seen.iter().any(|&b| b > 130.0 && b < 175.0), "and passes through the tempos between: {seen:?}");
    assert_eq!(*seen.last().unwrap(), 180.0, "and ends at the tempo asked for");
    assert!(s.pending().is_empty());
}

#[test]
fn a_tempo_ramp_replays_to_the_same_audio() {
    let mut s = ready();
    s.eval("lead.play([c4 e4 g4 b4])", Some("now")).unwrap();
    let mut original = run(&mut s, 20_000);
    s.eval("tempo 200bpm @ now over 2 beats", Some("now")).unwrap();
    original.extend(run(&mut s, 120_000));
    let log = from_lines(&to_lines(s.log())).unwrap();
    let replayed = Session::replay(SR, Layout::Mono, QuantizeDefaults::default(), &log, original.len() as u64, false);
    assert_eq!(original, replayed[0]);
    assert_eq!(s.transport().bpm, 200.0);
}

#[test]
fn a_ramp_that_is_too_long_is_refused() {
    let mut s = ready();
    let problems = s.eval("tempo 200bpm over 1000 bars", Some("now")).unwrap_err();
    assert!(problems[0].message.contains("a tempo ramp takes"), "{}", problems[0].message);
}

#[test]
fn a_change_waiting_for_a_bar_line_stays_on_it_when_the_meter_changes_first() {
    let mut s = ready();
    s.eval("lead.play([c4 e4 g4 b4])", Some("now")).unwrap();
    // the meter lands at the next bar (2s), the mute is meant for the one after (4s at 4/4)
    s.eval("lead.mute", Some("next 2 bars")).unwrap();
    s.eval("meter 2/4", Some("next bar")).unwrap();
    assert_eq!(s.pending().iter().map(|p| p.lands_at).collect::<Vec<_>>(), [96_000, 192_000]);
    run(&mut s, 97_000);
    // the meter landed at 2s, and the mute was a bar and a half after that: now bars are a second, so half that
    let waiting: Vec<_> = s.pending().iter().map(|p| p.lands_at).collect();
    assert_eq!(waiting, [96_000 + 48_000], "the bar line it was waiting for, now a bar closer");
}

#[test]
fn cancelling_a_ramp_by_its_id_cancels_every_step_of_it() {
    let mut s = ready();
    let ramp = s.eval("tempo 180bpm @ next bar over 2 bars", Some("now")).unwrap();
    s.eval("lead.mute", Some("next 4 bars")).unwrap();
    assert_eq!(s.pending().len(), 33, "32 steps and the mute");
    assert_eq!(s.cancel(ramp.id), 32);
    assert_eq!(s.pending().len(), 1);
    // one that is already under way goes the same way, from any of its steps
    let again = s.eval("tempo 180bpm @ next bar over 2 bars", Some("now")).unwrap();
    run(&mut s, 2 * BAR);
    let left = s.pending().len();
    assert!(left > 1 && left < 33, "{left}");
    assert_eq!(s.cancel(again.id), left - 1);
    assert_eq!(s.pending().len(), 1);
}

const EXPRESSIVE: &str = "
tempo 120bpm
instr organ(freq: hz, bend: st = 0st, pressure: 0..1 = 0) { osc(sine, freq + bend) * (0.2 + pressure * 0.6) * env[1 | 10ms 0] }
clip pad { instrument: organ
  keys: midi.keys(channel: 1) }
pad.start
";

#[test]
fn pitch_bend_moves_a_held_note_and_the_next_one_starts_bent() {
    use niminal_daemon::Expression::Bend;
    let mut s = session();
    s.eval(EXPRESSIVE, Some("now")).unwrap();
    s.key_input(1, 69, 100);
    let steady = run(&mut s, 9600);
    assert!((hz(&steady[2000..]) - 440.0).abs() < 12.0);
    assert_eq!(s.expression_input(1, Bend, 2.0, None), 1);
    let up = run(&mut s, 9600);
    assert!((hz(&up[2000..]) - 493.9).abs() < 14.0, "two semitones up: {}", hz(&up[2000..]));
    s.key_input(1, 69, 0);
    run(&mut s, 4800);
    s.key_input(1, 60, 100);
    let next = run(&mut s, 9600);
    assert!((hz(&next[2000..]) - 293.7).abs() < 12.0, "c4 bent up to d4: {}", hz(&next[2000..]));
    assert_eq!(s.expression_input(2, Bend, 1.0, None), 0, "another channel has no keys");
}

#[test]
fn pressure_changes_the_level_of_the_note_it_is_on() {
    use niminal_daemon::Expression::Pressure;
    let mut s = session();
    s.eval(EXPRESSIVE, Some("now")).unwrap();
    s.key_input(1, 69, 100);
    s.key_input(1, 72, 100);
    let soft = peak(&run(&mut s, 9600)[2000..]);
    assert_eq!(s.expression_input(1, Pressure, 1.0, Some(69)), 1, "one key's pressure reaches that key only");
    let a = run(&mut s, 9600);
    assert!(peak(&a[2000..]) > soft * 1.25, "{} vs {soft}", peak(&a[2000..]));
    assert_eq!(s.expression_input(1, Pressure, 0.5, None), 2);
}

#[test]
fn the_sustain_pedal_holds_notes_whose_keys_have_come_up() {
    use niminal_daemon::Expression::Sustain;
    let mut s = session();
    s.eval(EXPRESSIVE, Some("now")).unwrap();
    s.expression_input(1, Sustain, 1.0, None);
    s.key_input(1, 69, 100);
    s.key_input(1, 69, 0);
    assert!(peak(&run(&mut s, 2 * BAR)) > 0.05, "still sounding with the key up");
    assert_eq!(s.expression_input(1, Sustain, 0.0, None), 1, "the pedal comes up and lets it go");
    run(&mut s, 2400);
    assert!(peak(&run(&mut s, BAR)) < 1e-3);
    // pressing the same key again with the pedal down damps the old note
    s.expression_input(1, Sustain, 1.0, None);
    s.key_input(1, 69, 100);
    s.key_input(1, 69, 0);
    s.key_input(1, 69, 100);
    s.key_input(1, 69, 0);
    s.expression_input(1, Sustain, 0.0, None);
    run(&mut s, 2400);
    assert!(peak(&run(&mut s, BAR)) < 1e-3);
}

#[test]
fn expression_replays_to_the_same_audio() {
    use niminal_daemon::Expression::{Bend, Pressure, Sustain};
    let mut s = session();
    s.eval(EXPRESSIVE, Some("now")).unwrap();
    let mut original = run(&mut s, 1000);
    s.key_input(1, 69, 100);
    original.extend(run(&mut s, 2000));
    s.expression_input(1, Bend, 1.5, None);
    original.extend(run(&mut s, 1500));
    s.expression_input(1, Pressure, 0.8, None);
    s.expression_input(1, Sustain, 1.0, None);
    s.key_input(1, 69, 0);
    original.extend(run(&mut s, 5000));
    s.expression_input(1, Sustain, 0.0, None);
    original.extend(run(&mut s, 9000));
    let replayed = Session::replay(SR, Layout::Mono, QuantizeDefaults::default(), s.log(), original.len() as u64, false);
    assert_eq!(original, replayed[0]);
}

#[test]
fn new_forgets_the_old_piece_so_another_can_start() {
    let mut s = ready();
    s.eval("lead.play([c4 e4])", Some("now")).unwrap();
    run(&mut s, 4_800);
    s.eval("new", Some("now")).unwrap();
    let silent = run(&mut s, BAR);
    assert!(peak(&silent) < 1e-6, "everything stopped");
    assert!(s.tracks().iter().all(|t| t.name != "lead" && t.name != "bass"), "the old tracks are gone");
    s.eval("instr pluck(freq: hz) { osc(saw, freq) }\ntrack drone { instrument = pluck }\ndrone.play([a2])", Some("now")).unwrap();
    assert!(peak(&run(&mut s, BAR)) > 1e-3, "the new piece plays");
    assert_eq!(s.tracks().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["drone"]);
}

#[test]
fn new_and_the_next_piece_can_arrive_together() {
    let mut s = ready();
    s.eval("new\ninstr pluck(freq: hz) { osc(saw, freq) }\ntrack drone { instrument = pluck }\ndrone.play([a2])", Some("now")).unwrap();
    assert!(peak(&run(&mut s, BAR)) > 1e-3);
    assert_eq!(s.tracks().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["drone"]);
}

#[test]
fn an_envelope_lane_moves_a_track_parameter_as_the_clip_plays() {
    let mut s = session();
    s.eval("tempo 120bpm\ninstr saw1(freq: hz) { osc(saw, freq) * env[1 | 20ms 0] }\ntrack pad(cut: hz = 300hz) { instrument = saw1\n out = it.lpf(cutoff: cut) }", Some("now")).unwrap();
    s.eval("clip c { notes: [c5]\ncut: env[300hz 1bar 1300hz] }\npad.play(c)", Some("now")).unwrap();
    let cut = |s: &Session| s.control_values().into_iter().find(|v| v.0 == "pad::cut").unwrap().1;
    run(&mut s, 1_000);
    assert!((cut(&s) - 300.0).abs() < 20.0, "{}", cut(&s));
    run(&mut s, BAR / 2 - 1_000);
    assert!((cut(&s) - 800.0).abs() < 30.0, "half way up: {}", cut(&s));
    run(&mut s, BAR);
    assert!((cut(&s) - 1300.0).abs() < 1.0, "it holds the last level: {}", cut(&s));
}

#[test]
fn a_signal_lane_moves_a_track_parameter_continuously_even_without_notes() {
    let mut s = session();
    s.eval("tempo 120bpm\ninstr saw1(freq: hz) { osc(saw, freq) * env[1 | 20ms 0] }\ntrack pad(cut: hz = 300hz) { instrument = saw1\n out = it.lpf(cutoff: cut) }", Some("now")).unwrap();
    s.eval("clip c { cut: saw.range(300hz, 1300hz) }\npad.play(c)", Some("now")).unwrap();
    let cut = |s: &Session| s.control_values().into_iter().find(|v| v.0 == "pad::cut").unwrap().1;
    run(&mut s, 1_000);
    assert!((cut(&s) - 300.0).abs() < 60.0, "{}", cut(&s));
    run(&mut s, BAR / 2 - 1_000);
    assert!((cut(&s) - 800.0).abs() < 60.0, "half way up: {}", cut(&s));
    run(&mut s, BAR / 4);
    assert!((cut(&s) - 1050.0).abs() < 60.0, "three quarters: {}", cut(&s));
    run(&mut s, BAR / 4 + 1_000);
    assert!(cut(&s) < 400.0, "the saw starts over with the next bar: {}", cut(&s));
}

#[test]
fn set_can_give_a_part_a_track_parameter_lane_and_a_signal() {
    let mut s = session();
    s.eval("tempo 120bpm\ninstr saw1(freq: hz) { osc(saw, freq) * env[1 | 20ms 0] }\ntrack pad(cut: hz = 300hz) { instrument = saw1\n out = it.lpf(cutoff: cut) }", Some("now")).unwrap();
    s.eval("pad.play([c5])", Some("now")).unwrap();
    let cut = |s: &Session| s.control_values().into_iter().find(|v| v.0 == "pad::cut").unwrap().1;
    run(&mut s, 1_000);
    s.eval("pad.set(cut: 900hz)", Some("now")).unwrap();
    run(&mut s, BAR + 1_000);
    assert!((cut(&s) - 900.0).abs() < 1.0, "a note sets it as it starts: {}", cut(&s));
    s.eval("pad.set(cut: saw.range(300hz, 1300hz))", Some("now")).unwrap();
    run(&mut s, BAR / 2);
    assert!((cut(&s) - 800.0).abs() < 100.0, "and a signal moves it: {}", cut(&s));
}

mod mpe {
    use super::*;
    use niminal_daemon::ControlSignal;

    const ORGAN: &str = "tempo 120bpm\ninstr organ(freq: hz, bend: st = 0st, pressure: 0..1 = 0, slide: 0..1 = 0) { osc(sine, freq + bend) * (0.1 + pressure * 0.4 + slide * 0.4) * env[1 | 10ms 0] }\nclip pad { instrument: organ\n keys: midi.keys(mpe: true) }\npad.start";

    fn mpe_session() -> Session {
        let mut s = session();
        s.eval(ORGAN, Some("now")).unwrap();
        s
    }

    fn hz(x: &[f32]) -> f64 {
        crossings(x) as f64 / (x.len() as f64 / f64::from(SR))
    }

    fn bend(channel: u8, value: u16) -> ControlSignal<'static> {
        ControlSignal::MidiBend { channel, value }
    }

    #[test]
    fn every_member_channel_plays_and_the_master_channel_does_not() {
        let mut s = mpe_session();
        run(&mut s, 600);
        assert_eq!(s.key_input(1, 69, 100), 0, "the master channel carries no notes");
        assert_eq!(s.key_input(2, 69, 100), 1);
        assert_eq!(s.key_input(9, 72, 100), 1);
        assert!(peak(&run(&mut s, 2048)) > 0.05);
        // a plain keyboard clip still answers to its one channel
        let mut plain = session();
        plain.eval(&ORGAN.replace("mpe: true", "channel: 1"), Some("now")).unwrap();
        assert_eq!(plain.key_input(2, 69, 100), 0);
        assert_eq!(plain.key_input(1, 69, 100), 1);
    }

    #[test]
    fn a_note_is_bent_by_its_own_channel_over_forty_eight_semitones() {
        let mut s = mpe_session();
        run(&mut s, 600);
        s.key_input(2, 69, 100);
        s.key_input(3, 69, 100);
        run(&mut s, 2048);
        // a quarter of the way up the range is 12 semitones: an octave
        s.control_input(&bend(2, 8192 + 2048));
        let both = run(&mut s, 24_000);
        // the other note stays at 440, so the pair sound at 440 and 880 together
        let alone = {
            let mut one = mpe_session();
            run(&mut one, 600);
            one.key_input(2, 69, 100);
            run(&mut one, 2048);
            one.control_input(&bend(2, 8192 + 2048));
            run(&mut one, 24_000)
        };
        assert!((hz(&alone[4000..]) - 880.0).abs() < 20.0, "{}", hz(&alone[4000..]));
        assert!(rms(&both) > rms(&alone) * 1.1, "two notes are louder than one");
    }

    #[test]
    fn the_master_channel_moves_every_note_by_two_semitones_at_most() {
        let mut s = mpe_session();
        run(&mut s, 600);
        s.key_input(2, 69, 100);
        run(&mut s, 2048);
        s.control_input(&bend(1, 16383));
        let out = run(&mut s, 24_000);
        let expected = 440.0 * 2f64.powf(2.0 / 12.0);
        assert!((hz(&out[4000..]) - expected).abs() < 15.0, "{} vs {expected}", hz(&out[4000..]));
    }

    #[test]
    fn pressure_and_slide_reach_the_note_on_their_channel() {
        let level = |expression: ControlSignal| {
            let mut s = mpe_session();
            run(&mut s, 600);
            s.key_input(2, 69, 100);
            s.key_input(3, 69, 100);
            run(&mut s, 2048);
            s.control_input(&expression);
            peak(&run(&mut s, 4096))
        };
        let quiet = level(ControlSignal::MidiCc { cc: 1, channel: 2, value: 0 });
        assert!(level(ControlSignal::MidiPressure { channel: 2, note: None, value: 127 }) > quiet * 1.4);
        assert!(level(ControlSignal::MidiCc { cc: 74, channel: 3, value: 127 }) > quiet * 1.4);
    }

    #[test]
    fn the_pedal_on_the_master_channel_holds_every_note() {
        let mut s = mpe_session();
        run(&mut s, 600);
        s.key_input(2, 69, 100);
        s.key_input(3, 72, 100);
        run(&mut s, 2048);
        s.control_input(&ControlSignal::MidiCc { cc: 64, channel: 1, value: 127 });
        s.key_input(2, 69, 0);
        s.key_input(3, 72, 0);
        assert!(peak(&run(&mut s, 12_000)) > 0.05, "held by the pedal");
        s.control_input(&ControlSignal::MidiCc { cc: 64, channel: 1, value: 0 });
        run(&mut s, 4_000);
        assert!(peak(&run(&mut s, 8_000)) < 1e-3, "and let go with it");
    }

    #[test]
    fn mpe_keys_cannot_name_a_channel() {
        let mut s = session();
        let err = s.eval(&ORGAN.replace("mpe: true", "mpe: true, channel: 1"), Some("now")).unwrap_err();
        assert!(err[0].message.contains("takes no `channel:`"), "{}", err[0].message);
    }
}

mod quantize_settings {
    use super::*;

    #[test]
    fn a_projects_config_sets_when_each_kind_of_change_lands() {
        let mut s = ready();
        s.eval("config { quantize: { play: beat\n tempo: 2 bars\n default: 4 beats } }", Some("now")).unwrap();
        run(&mut s, 10_000);
        // starting a clip now waits for the next beat (24000 samples), not the next bar
        let play = s.eval("lead.play([c4])", None).unwrap();
        assert_eq!(play.lands_at, 24_000);
        // other commands take `default`: four beats is a bar
        let mute = s.eval("lead.mute", None).unwrap();
        assert_eq!(mute.lands_at, BAR as u64);
        let tempo = s.eval("tempo 100bpm", None).unwrap();
        assert_eq!(tempo.lands_at, 2 * BAR as u64, "a tempo change waits for two bars");
    }

    #[test]
    fn a_definition_can_say_when_it_lands() {
        let mut s = ready();
        run(&mut s, 10_000);
        // a track redefinition lands on the bar unless it says otherwise
        let plain = s.eval("track fx { instrument = pluck }", None).unwrap();
        assert_eq!(plain.lands_at, BAR as u64);
        let now = s.eval("track fx2 { instrument = pluck } with(quantize: now)", None).unwrap();
        assert_eq!(now.lands_at, 10_000, "it says now");
        let beat = s.eval("clip riff { notes: [c4] } with(quantize: 1 beat)", None).unwrap();
        assert_eq!(beat.lands_at, 24_000);
        let instr = s.eval("instr organ(freq: hz) with(quantize: 2 beats) { osc(sine, freq) }", None).unwrap();
        assert_eq!(instr.lands_at, 48_000);
        // the evaluation's own setting still wins over the definition's
        let over = s.eval("track fx4 { instrument = pluck } with(quantize: now)", Some("next 4 bars")).unwrap();
        assert_eq!(over.lands_at, 4 * BAR as u64);
    }

    #[test]
    fn quantum_mistakes_are_reported() {
        let mut s = ready();
        let message = |s: &mut Session, src: &str| s.eval(src, Some("now")).unwrap_err()[0].message.clone();
        assert_eq!(message(&mut s, "config { quantize: { sometimes: bar } }"), "unknown kind of change `sometimes`");
        assert_eq!(message(&mut s, "config { quantize: { play: soon } }"), "expected when a change lands");
        assert_eq!(message(&mut s, "clip c { notes: [c4] } with(speed: 2)"), "there is no option `speed` here");
        assert_eq!(message(&mut s, "track t2 { instrument = pluck } with(quantize: 2 hz)"), "expected when a change lands");
    }
}

#[test]
fn an_osc_out_tap_holds_the_latest_value_of_its_signal_while_audio_runs() {
    let mut s = session();
    s.eval("tempo 120bpm\ninstr tone(freq: hz) { osc(sine, freq) * env[1 | 5ms 0] }\ntrack lead {\n instrument = tone\n out = it.osc_out(\"/level\", port: 9000)\n}\nctl k = 0.25\ntrack ctl_track {\n out = k.osc_out(\"/knob\", port: 9000)\n}", Some("now")).unwrap();
    assert_eq!(s.program().taps.len(), 2);
    let (lead, knob) = (s.program().taps[0].clone(), s.program().taps[1].clone());
    run(&mut s, 2_000);
    assert_eq!(knob.value.read(), 0.25, "a control's value, as it is");
    assert_eq!(lead.value.read(), 0.0, "silence until a note");
    s.eval("lead.play([a4])", Some("now")).unwrap();
    let mut seen = 0.0f32;
    for _ in 0..40 {
        run(&mut s, 128);
        seen = seen.max(lead.value.read().abs());
    }
    assert!(seen > 0.3, "the track's signal reaches the tap: {seen}");
    s.eval("ctl k = 0.75", Some("now")).unwrap();
    run(&mut s, 4_000);
    assert!((s.program().taps[1].value.read() - 0.75).abs() < 1e-3, "{}", s.program().taps[1].value.read());
}

#[test]
fn a_session_can_be_given_audio_for_tracks_that_read_input() {
    let mut s = session();
    s.eval("tempo 120bpm\ntrack mic { out = input(1) * 0.5 }", Some("now")).unwrap();
    s.set_audio_input(vec![(0..10_000).map(|i| i as f32 / 20_000.0).collect()]);
    let out = run(&mut s, 12_000);
    assert!((0..10_000).all(|i| (out[i] - i as f32 / 40_000.0).abs() < 1e-6), "it hears the buffer from sample 0");
    assert!(out[10_000..].iter().all(|v| *v == 0.0), "and nothing after it ends");
}

#[test]
fn a_midi_out_instrument_sends_notes_with_their_times_and_stays_silent() {
    let mut s = session();
    s.eval("tempo 120bpm\nmidi_out synth(port: \"IAC\", channel: 3)\ntrack lead { instrument = synth }", Some("now")).unwrap();
    s.eval("lead.play([c4 e4 ~ g4])", Some("now")).unwrap();
    let out = run(&mut s, BAR);
    assert!(peak(&out) == 0.0, "the sound is outside");
    let events = s.take_midi();
    let shown: Vec<_> = events.iter().map(|e| (e.at, e.on, e.note, e.channel, &*e.port)).collect();
    // a bar is 96000 samples: four steps of 24000, the third a rest
    assert_eq!(
        shown,
        [(0, true, 60, 3, "IAC"), (24_000, false, 60, 3, "IAC"), (24_000, true, 64, 3, "IAC"), (48_000, false, 64, 3, "IAC"), (72_000, true, 67, 3, "IAC")],
        "{shown:?}"
    );
    assert!(s.take_midi().is_empty(), "taken once");
    // the last note ends with its step, as the clip starts over
    run(&mut s, 24_000);
    let rest = s.take_midi();
    assert_eq!(rest.iter().map(|e| (e.at, e.on, e.note)).collect::<Vec<_>>(), [(BAR as u64, false, 67), (BAR as u64, true, 60)]);
}

#[test]
fn panic_and_hush_end_midi_notes_that_are_sounding() {
    let mut s = session();
    s.eval("tempo 120bpm\nmidi_out synth(port: \"IAC\")\ntrack lead { instrument = synth }\nlead.play([c4])", Some("now")).unwrap();
    run(&mut s, 1_000);
    assert_eq!(s.take_midi().len(), 1);
    s.eval("panic", Some("now")).unwrap();
    run(&mut s, 100);
    let events = s.take_midi();
    assert_eq!(events.iter().map(|e| (e.on, e.note)).collect::<Vec<_>>(), [(false, 60)], "{events:?}");
}
