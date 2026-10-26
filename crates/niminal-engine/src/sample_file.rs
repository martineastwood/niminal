//! Audio and numeric file decoding shared by all frontends.

use crate::Table;
use std::path::Path;

/// Decode a sample file to one table per channel, preserving its sample rate.
pub fn read_sample_file(path: &Path) -> Result<Vec<Table>, String> {
    let is_text = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "csv" | "txt" | "dat"));
    let is_wav = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("wav"));
    if is_text {
        read_text(path)
    } else if is_wav {
        read_wav(path)
    } else {
        read_compressed(path)
    }
}

/// The kinds of audio file a sample can be, by extension: WAV, and what the decoder reads besides.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "wav", "flac", "mp3", "ogg", "oga", "aif", "aiff", "aifc", "m4a", "mp4", "aac", "caf",
];

/// Whether `path` names a file that `sample(...)` can load as audio.
pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// A text file of numbers, a row per line and a column per channel, split by commas, tabs or
/// spaces. `#` starts a comment, and a first line that isn't numbers is a header.
fn read_text(path: &Path) -> Result<Vec<Table>, String> {
    let fail = |e: &dyn std::fmt::Display| format!("can't read {}: {e}", path.display());
    let text = std::fs::read_to_string(path).map_err(|e| fail(&e))?;
    let mut columns: Vec<Vec<f32>> = Vec::new();
    let mut seen_row = false;
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line
            .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
            .filter(|f| !f.is_empty())
            .collect();
        let row: Result<Vec<f32>, _> = fields.iter().map(|f| f.parse::<f32>()).collect();
        let Ok(row) = row else {
            if seen_row {
                return Err(fail(&format!("line {} is not all numbers", n + 1)));
            }
            seen_row = true; // a header
            continue;
        };
        seen_row = true;
        if columns.is_empty() {
            columns = vec![Vec::new(); row.len()];
        } else if row.len() != columns.len() {
            return Err(fail(&format!(
                "line {} has {} values, the lines before it have {}",
                n + 1,
                row.len(),
                columns.len()
            )));
        }
        for (column, x) in columns.iter_mut().zip(row) {
            column.push(x);
        }
    }
    if columns.is_empty() {
        return Err(format!("{} has no numbers in it", path.display()));
    }
    Ok(columns
        .into_iter()
        .map(|c| Table::from_f32(c.into_boxed_slice(), None))
        .collect())
}

/// FLAC, MP3, Ogg Vorbis, AIFF, AAC and the like, decoded to one table per channel.
fn read_compressed(path: &Path) -> Result<Vec<Table>, String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let fail = |e: &dyn std::fmt::Display| format!("can't read {}: {e}", path.display());
    let file = std::fs::File::open(path).map_err(|e| fail(&e))?;
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            MediaSourceStream::new(Box::new(file), Default::default()),
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| fail(&e))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| fail(&"it has no audio track"))?;
    let (track_id, params) = (track.id, track.codec_params.clone());
    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| fail(&e))?;
    let mut buffers: Vec<Vec<f32>> = Vec::new();
    let mut rate = params.sample_rate;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            // the end of the file arrives as an end-of-stream error
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(fail(&e)),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let channels = spec.channels.count();
                if buffers.is_empty() {
                    buffers = vec![Vec::new(); channels];
                    rate = Some(spec.rate);
                }
                let mut block = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                block.copy_interleaved_ref(decoded);
                for frame in block.samples().chunks_exact(channels) {
                    for (buffer, &x) in buffers.iter_mut().zip(frame) {
                        buffer.push(x);
                    }
                }
            }
            // a damaged packet is skipped, as a player would
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(fail(&e)),
        }
    }
    if buffers.iter().all(Vec::is_empty) {
        return Err(format!("{} has no audio in it", path.display()));
    }
    let rate = rate.map(|r| r as f32);
    Ok(buffers
        .into_iter()
        .map(|b| Table::from_f32(b.into_boxed_slice(), rate))
        .collect())
}

fn read_wav(path: &Path) -> Result<Vec<Table>, String> {
    let fail = |e: &dyn std::fmt::Display| format!("can't read {}: {e}", path.display());
    let mut reader = hound::WavReader::open(path).map_err(|e| fail(&e))?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels);
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| fail(&e))?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1_u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(|e| fail(&e))?
        }
    };
    if channels == 0 || interleaved.is_empty() {
        return Err(format!("{} has no audio in it", path.display()));
    }
    let mut buffers = vec![Vec::with_capacity(interleaved.len() / channels); channels];
    for frame in interleaved.chunks_exact(channels) {
        for (buffer, &x) in buffers.iter_mut().zip(frame) {
            buffer.push(x);
        }
    }
    let rate = Some(spec.sample_rate as f32);
    Ok(buffers
        .into_iter()
        .map(|b| Table::from_f32(b.into_boxed_slice(), rate))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flac_preserves_channels_samples_and_rate() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stereo.flac");
        let tables = read_sample_file(&path).unwrap();
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].len(), 4);
        assert_eq!(tables[0].sample_rate_or(0.0), 44100.0);
        assert_eq!(tables[0].get(1), 0.5);
        assert_eq!(tables[1].get(0), 0.5);
        assert_eq!(tables[1].get(3), -1.0);
    }
}
