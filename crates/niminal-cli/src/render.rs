//! Writing audio to files.

pub const SAMPLE_RATE: u32 = 48_000;
pub const DEFAULT_SAMPLE_RATE: u32 = SAMPLE_RATE;

/// Write the channels as an interleaved 32-bit float WAV at `sample_rate`.
pub fn write_wav(
    path: &std::path::Path,
    channels: &[Vec<f32>],
    sample_rate: u32,
) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels: channels.len() as u16,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    let len = channels.first().map_or(0, Vec::len);
    for i in 0..len {
        for c in channels {
            writer.write_sample(c[i])?;
        }
    }
    writer.finalize()
}

#[cfg(test)]
mod tests {
    use super::write_wav;

    #[test]
    fn the_file_says_the_rate_it_was_made_at() {
        let path = std::env::temp_dir().join(format!("niminal-wav-rate-{}.wav", std::process::id()));
        let audio = vec![vec![0.5f32; 100], vec![-0.5f32; 100]];
        for rate in [44_100, 48_000, 96_000] {
            write_wav(&path, &audio, rate).unwrap();
            let reader = hound::WavReader::open(&path).unwrap();
            assert_eq!(reader.spec().sample_rate, rate);
            assert_eq!(reader.spec().channels, 2);
            assert_eq!(reader.duration(), 100);
        }
        let _ = std::fs::remove_file(path);
    }
}
