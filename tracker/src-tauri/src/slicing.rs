//! Finding where the hits of a sample begin, so it can be cut there instead of into equal pieces.

/// The shares of the sample's length (0 up to 1) at which pieces start: always 0 first, then each
/// transient, at most `max` of them in all, at least 60 ms apart (`sensitivity` 0 to 1, 0.5 the usual: higher finds softer hits). Silence or a steady sound give just 0.
pub fn transients(samples: &[f32], rate: f32, max: usize, sensitivity: f32) -> Vec<f64> {
    niminal_lang::detect_transients_at(samples, rate, max, sensitivity).into_iter()
        .map(|frame| frame as f64 / samples.len().max(1) as f64).collect()
}

/// The places to cut `file` (relative to the project folder `dir`) at, from [`transients`].
pub fn detect(dir: &std::path::Path, file: &str, max: usize, sensitivity: f32) -> Result<Vec<f64>, String> {
    let (mixed, rate) = mono(dir, file)?;
    Ok(transients(&mixed, rate, max, sensitivity))
}

/// A sample as mixed to mono: its frames and rate, and the lowest and highest value in each of `buckets`
/// equal stretches of it (two numbers a bucket), for drawing its waveform.
#[derive(serde::Serialize)]
pub struct Peaks {
    pub frames: usize,
    pub rate: f32,
    pub peaks: Vec<f32>,
}

/// The waveform of `file` (relative to the project folder `dir`) in `buckets` stretches.
pub fn peaks(dir: &std::path::Path, file: &str, buckets: usize) -> Result<Peaks, String> {
    let (mixed, rate) = mono(dir, file)?;
    let buckets = buckets.clamp(1, 8192);
    let mut peaks = Vec::with_capacity(buckets * 2);
    for bucket in 0..buckets {
        let from = bucket * mixed.len() / buckets;
        let to = ((bucket + 1) * mixed.len() / buckets).max(from + 1).min(mixed.len());
        let part = mixed.get(from..to).unwrap_or(&[]);
        peaks.push(part.iter().copied().fold(0.0, f32::min));
        peaks.push(part.iter().copied().fold(0.0, f32::max));
    }
    Ok(Peaks { frames: mixed.len(), rate, peaks })
}

/// How long `file` plays, in seconds.
pub fn seconds(dir: &std::path::Path, file: &str) -> Result<f64, String> {
    let (mixed, rate) = mono(dir, file)?;
    Ok(mixed.len() as f64 / f64::from(rate))
}

fn mono(dir: &std::path::Path, file: &str) -> Result<(Vec<f32>, f32), String> {
    let channels = niminal_lang::Samples::new(dir).load(file)?;
    let first = channels.first().ok_or("The file has no audio")?;
    let mixed = (0..first.len()).map(|i| channels.iter().map(|c| c.get(i)).sum::<f32>() / channels.len() as f32).collect();
    Ok((mixed, first.sample_rate_or(48_000.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    /// A second of silence with a decaying burst of sine and noise starting at each of `starts` (in seconds).
    fn hits(starts: &[f32]) -> Vec<f32> {
        let mut audio = vec![0.0; RATE as usize];
        let mut noise = 12345u32;
        for &start in starts {
            for n in 0..(RATE * 0.2) as usize {
                let t = n as f32 / RATE;
                noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let hiss = (noise >> 16) as f32 / 32768.0 - 1.0;
                let at = (start * RATE) as usize + n;
                if at < audio.len() {
                    audio[at] += ((t * 220.0 * std::f32::consts::TAU).sin() * 0.7 + hiss * 0.3) * (-t * 25.0).exp();
                }
            }
        }
        audio
    }

    #[test]
    fn finds_where_each_hit_begins() {
        let found = transients(&hits(&[0.0, 0.25, 0.5, 0.8]), RATE, 64, 0.5);
        assert_eq!(found.len(), 4, "{found:?}");
        for (share, expected) in found.iter().zip([0.0, 0.25, 0.5, 0.8]) {
            assert!((share - expected).abs() < 0.01, "{found:?}");
        }
        // the cut falls just before the hit, not after it
        assert!(found[2] <= 0.5 && found[2] > 0.49, "{found:?}");
    }

    #[test]
    fn a_higher_sensitivity_finds_softer_hits() {
        // a loud hit, then one a tenth as loud, then one a twentieth as loud
        let mut audio = hits(&[0.0, 0.3]);
        let soft = hits(&[0.6]);
        let softer = hits(&[0.8]);
        for (i, sample) in audio.iter_mut().enumerate() {
            *sample += soft[i] * 0.07 + softer[i] * 0.03;
        }
        let count = |sensitivity| transients(&audio, RATE, 64, sensitivity).len();
        assert!(count(0.0) <= count(0.5) && count(0.5) <= count(1.0), "{} {} {}", count(0.0), count(0.5), count(1.0));
        assert!(count(1.0) > count(0.5), "{} {}", count(0.5), count(1.0));
        assert_eq!(count(0.5), transients(&audio, RATE, 64, 0.5).len());
    }

    #[test]
    fn the_waveform_is_the_lowest_and_highest_of_each_stretch() {
        let dir = std::env::temp_dir().join(format!("niminal-tracker-peaks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 8 frames of 16-bit mono: 0, 8192, -16384, 0, 0, 16384, 0, 0
        let values: [i16; 8] = [0, 8192, -16384, 0, 0, 16384, 0, 0];
        let mut wav = Vec::new();
        wav.extend(b"RIFF"); wav.extend((36u32 + 16).to_le_bytes()); wav.extend(b"WAVEfmt "); wav.extend(16u32.to_le_bytes());
        wav.extend([1, 0, 1, 0]); wav.extend(48000u32.to_le_bytes()); wav.extend(96000u32.to_le_bytes()); wav.extend([2, 0, 16, 0]);
        wav.extend(b"data"); wav.extend(16u32.to_le_bytes()); wav.extend(values.iter().flat_map(|v| v.to_le_bytes()));
        std::fs::write(dir.join("a.wav"), wav).unwrap();
        let peaks = peaks(&dir, "a.wav", 2).unwrap();
        assert_eq!((peaks.frames, peaks.rate), (8, 48000.0));
        assert_eq!(peaks.peaks, [-0.5, 0.25, 0.0, 0.5]);
        assert_eq!(seconds(&dir, "a.wav").unwrap(), 8.0 / 48000.0);
        assert_eq!(super::peaks(&dir, "a.wav", 100).unwrap().peaks.len(), 200);
        assert!(super::peaks(&dir, "missing.wav", 4).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hit_after_a_silent_start_leaves_the_start_as_the_first_piece() {
        let found = transients(&hits(&[0.3, 0.6]), RATE, 64, 0.5);
        assert_eq!(found.len(), 3, "{found:?}");
        assert_eq!(found[0], 0.0);
    }

    #[test]
    fn keeps_the_strongest_when_there_are_too_many_and_ignores_silence_and_steady_tones() {
        let found = transients(&hits(&[0.0, 0.2, 0.4, 0.6, 0.8]), RATE, 3, 0.5);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0], 0.0);
        assert_eq!(transients(&vec![0.0; 48_000], RATE, 16, 0.5), [0.0]);
        let steady: Vec<f32> = (0..48_000).map(|n| (n as f32 * 0.05).sin() * 0.5).collect();
        assert_eq!(transients(&steady, RATE, 16, 0.5), [0.0]);
        assert_eq!(transients(&[], RATE, 16, 0.5), [0.0]);
    }
}
