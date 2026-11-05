#![allow(clippy::unwrap_used)]
//! Render the preset shelf for listening, or rebuild the original starter sample pack.
//! cargo run -p niminal-lang --example audition -- target/audition
//! cargo run -p niminal-lang --example audition -- --starter crates/niminal-lang/assets/starter

use std::error::Error;
use std::fmt::Write as _;
use std::path::Path;

use niminal_engine::{Voice, block_size};
use niminal_lang::presets::{PresetKind, all};
use niminal_lang::{Instrument, compile};

const SR: u32 = 48_000;
const STARTER: &[&str] = &[
    "kick_808",
    "snare_tri",
    "clap",
    "hat_closed",
    "hat_open",
    "tom",
    "cowbell",
    "crash",
    "electric_piano",
    "metal_bar",
    "struck_string",
    "fm_keys",
];

fn render(instrument: &Instrument, pitches: &[f32], gain: f32, hold: f32) -> Vec<f32> {
    let frames = ((hold + 4.0) * SR as f32) as usize;
    let release = (hold * SR as f32) as usize;
    let mut mix = vec![0.0; frames];
    for &pitch in pitches {
        let mut voice = Voice::new(instrument.graph.clone(), SR as f32);
        voice.set_param_by_name("freq", pitch);
        voice.set_param_by_name("gain", gain);
        voice.start_note();
        let mut pos = 0;
        while pos < frames {
            if pos == release {
                voice.release();
            }
            let mut end = (pos + block_size()).min(frames);
            if pos < release {
                end = end.min(release);
            }
            let mut block = vec![0.0; end - pos];
            voice.process(&mut block);
            for (out, sample) in mix[pos..end].iter_mut().zip(block) {
                *out += sample / pitches.len() as f32;
            }
            pos = end;
        }
        assert!(
            !voice.is_poisoned(),
            "{} produced invalid audio",
            instrument.name
        );
    }
    assert!(
        mix.iter().all(|s| s.is_finite()),
        "{} produced invalid audio",
        instrument.name
    );
    mix
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |a, s| a.max(s.abs()))
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
        as f32
}

fn db(level: f32) -> f32 {
    20.0 * level.max(1e-9).log10()
}

fn write_wav(path: &Path, samples: &[f32], scale: f32) -> Result<(), Box<dyn Error>> {
    let mut wav = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: SR,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )?;
    for &sample in samples {
        wav.write_sample(((sample * scale).clamp(-1.0, 1.0) * 32767.0).round() as i16)?;
    }
    wav.finalize()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let first = args
        .next()
        .ok_or("usage: audition [--starter] OUTPUT_DIRECTORY")?;
    let starter = first == "--starter";
    let out = if starter {
        args.next().ok_or("--starter needs an output directory")?
    } else {
        first
    };
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let dir = Path::new(&out);
    std::fs::create_dir_all(dir)?;
    let mut report =
        String::from("preset,scenario,peak_dbfs,attack_rms_dbfs,tail_rms_dbfs,preview_gain_db\n");
    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><title>Niminal preset audition</title><style>body{font:16px system-ui;max-width:1000px;margin:3rem auto;padding:1rem}section{border-top:1px solid #ccc;padding:1rem 0}audio{display:block;width:100%}small{color:#555}</style><h1>Niminal preset audition</h1><p>Preview gain targets -20 dBFS RMS over the default note’s first 0.5s, with a -6 dBFS peak ceiling and +18 dB gain cap; it is fixed per preset. Soft notes retain their relative level. Raw measurements are in report.csv. Chords are averaged to the level of one voice. No master limiter or effects are used.</p>",
    );
    let scenarios = [
        ("default", 1.0, 1.0),
        ("soft", 0.25, 1.0),
        ("short", 1.0, 0.08),
        ("long", 1.0, 2.0),
    ];
    for preset in all().iter().filter(|p| p.kind == PresetKind::Instrument) {
        if starter && !STARTER.contains(&preset.id.as_str()) {
            continue;
        }
        let source = format!("{}\n{}", preset.helpers, preset.source);
        let program = compile(&source).map_err(|e| format!("{}: {e:?}", preset.id))?;
        let instrument = &program.instruments[0];
        let drum = preset.shelf == "std/drums";
        let pitch = if drum || starter {
            130.812_79
        } else {
            261.625_58
        };
        // std/drums uses c3 (130.81Hz) as its reference tuning.
        let default = render(instrument, &[pitch], 1.0, 1.0);
        // A sub-heavy 808 needs more peak headroom than midrange hits to sit at a useful playback level.
        let target_peak = if starter && preset.id == "kick_808" {
            0.8
        } else {
            0.5
        };
        let peak_gain = target_peak / peak(&default).max(1e-6);
        let preview_gain = if starter {
            peak_gain.min(8.0)
        } else {
            (0.1 / rms(&default[..24_000]).max(1e-6))
                .min(peak_gain)
                .min(8.0)
        };
        if starter {
            let mut samples = default;
            let end = samples.iter().rposition(|s| s.abs() > 1e-5).unwrap_or(0) + 1;
            samples.truncate((end + 480).min(samples.len()));
            // A 10ms fade ends every recording cleanly, including truncated physical tails.
            let fade = 480.min(samples.len());
            let start = samples.len() - fade;
            for (i, s) in samples[start..].iter_mut().enumerate() {
                *s *= 1.0 - i as f32 / (fade - 1).max(1) as f32;
            }
            write_wav(
                &dir.join(format!("{}.wav", preset.id)),
                &samples,
                preview_gain,
            )?;
            std::fs::write(
                dir.join(format!("{}.nml", preset.id)),
                format!(
                    "// Original starter recording source. Rebuild with the audition example's --starter option.\n{source}\n{}(freq: c3) for 1sec\n",
                    preset.id
                ),
            )?;
            continue;
        }
        for (scenario, gain, hold) in scenarios {
            let samples = if scenario == "default" {
                default.clone()
            } else {
                render(instrument, &[pitch], gain, hold)
            };
            let file = format!("{}-{scenario}.wav", preset.id);
            write_wav(&dir.join(&file), &samples, preview_gain)?;
            writeln!(
                report,
                "{},{scenario},{:.2},{:.2},{:.2},{:.2}",
                preset.id,
                db(peak(&samples)),
                db(rms(&samples[..24_000])),
                db(rms(&samples[samples.len() - 24_000..])),
                db(preview_gain)
            )?;
            write!(
                html,
                "<section><b>{} / {scenario}</b><small> — peak {:.1} dBFS, first 0.5s RMS {:.1} dBFS, preview gain {:+.1} dB</small><audio controls preload=none src='{file}'></audio></section>",
                preset.id,
                db(peak(&samples)),
                db(rms(&samples[..24_000])),
                db(preview_gain)
            )?;
        }
        if !drum {
            let samples = render(
                instrument,
                &[pitch, pitch * 1.259_921, pitch * 1.498_307],
                1.0,
                1.0,
            );
            let file = format!("{}-chord.wav", preset.id);
            write_wav(&dir.join(&file), &samples, preview_gain)?;
            write!(
                html,
                "<section><b>{} / chord</b><audio controls preload=none src='{file}'></audio></section>",
                preset.id
            )?;
        }
        std::fs::write(dir.join(format!("{}.nml", preset.id)), &source)?;
    }
    if starter {
        let mut kit =
            String::from("// Original synthesized starter recordings. All roots are c3.\n");
        for name in STARTER {
            let choke = if name.starts_with("hat_") {
                ".choke(hats)"
            } else {
                ""
            };
            writeln!(kit, "{name} = sample(\"{name}.wav\").root(c3){choke}")?;
        }
        kit.push_str("\ndrums = kit {\n  kick: kick_808\n  snare: snare_tri\n  clap: clap\n  hat: hat_closed\n  open: hat_open\n  tom: tom\n  cowbell: cowbell\n  crash: crash\n}\n");
        std::fs::write(dir.join("starter.nml"), kit)?;
    } else {
        std::fs::write(dir.join("report.csv"), report)?;
        std::fs::write(dir.join("index.html"), html)?;
    }
    println!(
        "wrote {} to {}",
        if starter {
            "starter recordings"
        } else {
            "audition and level report"
        },
        dir.display()
    );
    Ok(())
}
