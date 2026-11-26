//! `niminal render`: the renderer itself lives in the sequencer crate, so a browser build can use it; this adds
//! writing the result to a WAV file.

pub use niminal_sequencer::render::*;

/// Write the channels as an interleaved 32-bit float WAV.
pub fn write_wav(path: &std::path::Path, channels: &[Vec<f32>]) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels: channels.len() as u16,
        sample_rate: SAMPLE_RATE,
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
