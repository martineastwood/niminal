//! Sample preparation writes new files; existing audio remains available to undo.
use std::path::{Component, Path};
use std::hash::{Hash, Hasher};
use serde::{Deserialize, Serialize};

pub fn project_file(dir: &Path, file: &str) -> Result<std::path::PathBuf, String> {
    let relative = Path::new(file);
    if !relative.starts_with("samples") || relative.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err("Choose a file under the project's samples folder".into());
    }
    Ok(dir.join(relative))
}

#[derive(Serialize)]
pub struct Analysis {
    pub frames: usize,
    pub rate: f32,
    pub channels: usize,
    pub peak: f32,
    pub silence_start: f64,
    pub silence_end: f64,
    pub crossings: Vec<f64>,
    pub loop_start: f64,
    pub loop_end: f64,
}

pub fn analyze(dir: &Path, file: &str) -> Result<Analysis, String> {
    project_file(dir, file)?;
    let channels = niminal_lang::Samples::new(dir).load(file)?;
    let first = channels.first().ok_or("The file has no audio")?;
    let frames = first.len();
    if frames == 0 { return Err("The file has no frames".into()); }
    let mixed: Vec<f32> = (0..frames).map(|i| channels.iter().map(|c| c.get(i)).filter(|v| v.is_finite()).max_by(|a, b| a.abs().total_cmp(&b.abs())).unwrap_or(0.0)).collect();
    let peak = channels.iter().flat_map(|c| (0..c.len()).map(|i| c.get(i).abs())).filter(|v| v.is_finite()).fold(0.0, f32::max);
    let audible = |i| channels.iter().any(|c| c.get(i).is_finite() && c.get(i).abs() > peak * 0.001);
    let start = (0..frames).find(|&i| audible(i)).unwrap_or(0);
    let end = (0..frames).rfind(|&i| audible(i)).map_or(frames, |i| i + 1);
    let crossings: Vec<usize> = (1..frames).filter(|&i| mixed[i - 1] <= 0.0 && mixed[i] > 0.0).collect();
    // Compare samples and slopes around positive crossings, preferring a long sustain region.
    let candidates: Vec<_> = crossings.iter().copied().filter(|&i| i > start.max(frames / 10) && i < end.min(frames * 9 / 10)).collect();
    let stride = (candidates.len() / 256).max(1);
    let mut best = (f32::INFINITY, frames / 4, (frames * 3 / 4).max(frames / 4 + 1));
    for &a in candidates.iter().step_by(stride).take(256) {
        for &b in candidates.iter().rev().step_by(stride).take(256) {
            if b <= a + frames / 4 { continue; }
            let radius = 32.min(a).min(frames - b);
            if radius == 0 || mixed[a..a + radius].iter().all(|v| v.abs() < peak * 0.01) { continue; }
            let score = channels.iter().map(|c| (0..radius).map(|i| (c.get(a + i) - c.get(b + i)).powi(2)).sum::<f32>()).sum::<f32>() / radius as f32;
            if score < best.0 { best = (score, a, b); }
        }
    }
    Ok(Analysis { frames, rate: first.sample_rate_or(48_000.0), channels: channels.len(), peak,
        silence_start: start as f64 / frames as f64, silence_end: end as f64 / frames as f64,
        crossings: crossings.into_iter().map(|i| i as f64 / frames as f64).collect(),
        loop_start: best.1 as f64 / frames as f64, loop_end: best.2 as f64 / frames as f64 })
}

#[derive(Deserialize)]
pub struct Edit {
    pub start: f64,
    pub end: f64,
    pub gain_db: f64,
    pub reverse: bool,
    pub normalize: bool,
    pub fade_in_ms: f64,
    pub fade_out_ms: f64,
}

pub fn prepare(dir: &Path, file: &str, edit: Edit) -> Result<String, String> {
    project_file(dir, file)?;
    if ![edit.start, edit.end, edit.gain_db, edit.fade_in_ms, edit.fade_out_ms].iter().all(|v| v.is_finite())
        || edit.start < 0.0 || edit.end > 1.0 || edit.start >= edit.end || !(-60.0..=24.0).contains(&edit.gain_db)
        || !(0.0..=60_000.0).contains(&edit.fade_in_ms) || !(0.0..=60_000.0).contains(&edit.fade_out_ms) {
        return Err("Invalid sample edit range, gain or fade".into());
    }
    let channels = niminal_lang::Samples::new(dir).load(file)?;
    let first = channels.first().ok_or("The file has no audio")?;
    let from = (edit.start * first.len() as f64).round() as usize;
    let to = (edit.end * first.len() as f64).round() as usize;
    if to <= from { return Err("Select at least one frame".into()); }
    let rate = first.sample_rate_or(48_000.0);
    let frames = to - from;
    let peak = channels.iter().flat_map(|c| (from..to).map(|i| c.get(i).abs())).filter(|v| v.is_finite()).fold(0.0, f32::max);
    let gain = 10.0_f64.powf(edit.gain_db / 20.0) * if edit.normalize && peak > 0.0 { 0.891_250_9 / f64::from(peak) } else { 1.0 };
    let fade_in = (edit.fade_in_ms * f64::from(rate) / 1000.0).min(frames as f64) as usize;
    let fade_out = (edit.fade_out_ms * f64::from(rate) / 1000.0).min(frames as f64) as usize;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
    let relative = format!("samples/edit_{stamp}.wav");
    let path = dir.join(&relative);
    let output = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut writer = hound::WavWriter::new(output, hound::WavSpec { channels: channels.len() as u16, sample_rate: rate as u32, bits_per_sample: 32, sample_format: hound::SampleFormat::Float }).map_err(|e| e.to_string())?;
        for i in 0..frames {
            let at = if edit.reverse { to - 1 - i } else { from + i };
            let envelope = if fade_in > 0 { (i as f32 / fade_in as f32).min(1.0) } else { 1.0 }
                * if fade_out > 0 { ((frames - 1 - i) as f32 / fade_out as f32).min(1.0) } else { 1.0 };
            for channel in &channels {
                let value = (f64::from(channel.get(at)) * gain * f64::from(envelope)) as f32;
                writer.write_sample(if value.is_finite() { value } else { 0.0 }).map_err(|e| e.to_string())?;
            }
        }
        writer.finalize().map_err(|e| e.to_string())
    })();
    if result.is_err() { let _ = std::fs::remove_file(path); }
    result.map(|()| relative)
}

#[derive(Serialize)]
pub struct Asset {
    pub file: String,
    pub frames: Option<usize>,
    pub rate: Option<f32>,
    pub channels: Option<usize>,
    pub duplicate_of: Option<String>,
    pub error: Option<String>,
}

pub fn assets(dir: &Path) -> Vec<Asset> {
    let paths = niminal_lang::files_under(&dir.join("samples"), niminal_lang::is_audio);
    let mut contents: Vec<(String, u64, usize)> = Vec::new();
    let samples = niminal_lang::Samples::new(dir);
    paths.into_iter().filter_map(|path| {
        let file = Path::new(&path).strip_prefix(dir).ok()?.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(&path).unwrap_or_default();
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hash);
        let hash = hash.finish();
        let duplicate_of = contents.iter().find(|(f, h, size)| !bytes.is_empty() && *h == hash && *size == bytes.len() && std::fs::read(dir.join(f)).is_ok_and(|b| b == bytes)).map(|(f, _, _)| f.clone());
        if duplicate_of.is_none() { contents.push((file.clone(), hash, bytes.len())); }
        Some(match samples.load(&file) {
            Ok(channels) => Asset { file, frames: channels.first().map(|c| c.len()), rate: channels.first().map(|c| c.sample_rate_or(48_000.0)), channels: Some(channels.len()), duplicate_of, error: None },
            Err(error) => Asset { file, frames: None, rate: None, channels: None, duplicate_of, error: Some(error) },
        })
    }).collect()
}

pub fn kit_members(dir: &Path, folder: &str) -> Result<Vec<String>, String> {
    let path = project_file(dir, folder)?;
    let mut members: Vec<String> = std::fs::read_dir(path).map_err(|e| e.to_string())?.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()) && niminal_lang::is_audio(&e.path()))
        .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().into_owned())).collect();
    members.sort();
    Ok(members)
}

pub fn replace_kit_member(dir: &Path, folder: &str, member: &str, replacement: &str) -> Result<String, String> {
    let path = project_file(dir, folder)?;
    if !kit_members(dir, folder)?.iter().any(|m| m == member) { return Err("No such kit member".into()); }
    let rendered = prepare(dir, replacement, Edit { start: 0.0, end: 1.0, gain_db: 0.0, reverse: false, normalize: false, fade_in_ms: 0.0, fade_out_ms: 0.0 })?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
    let relative = format!("samples/kit_edit_{stamp}");
    let target = dir.join(&relative);
    std::fs::create_dir(&target).map_err(|e| e.to_string())?;
    let result = (|| {
        for entry in std::fs::read_dir(path).map_err(|e| e.to_string())?.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_file()) || !niminal_lang::is_audio(&entry.path()) { continue; }
            if entry.path().file_stem().is_some_and(|s| s == member) {
                std::fs::copy(dir.join(&rendered), target.join(format!("{member}.wav"))).map_err(|e| e.to_string())?;
            } else { std::fs::copy(entry.path(), target.join(entry.file_name())).map_err(|e| e.to_string())?; }
        }
        Ok(relative)
    })();
    let _ = std::fs::remove_file(dir.join(rendered));
    if result.is_err() { let _ = std::fs::remove_dir_all(target); }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("niminal-sample-tools-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("samples")).unwrap(); dir
    }
    fn write(dir: &Path, file: &str, values: &[[f32; 2]]) {
        let mut w = hound::WavWriter::create(dir.join(file), hound::WavSpec { channels: 2, sample_rate: 8000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float }).unwrap();
        for frame in values { for value in frame { w.write_sample(*value).unwrap(); } } w.finalize().unwrap();
    }
    fn edit() -> Edit { Edit { start: 0.0, end: 1.0, gain_db: 0.0, reverse: false, normalize: false, fade_in_ms: 0.0, fade_out_ms: 0.0 } }
    #[test]
    fn preparation_keeps_stereo_and_originals_and_handles_trim_reverse_gain_and_fades() {
        let dir = folder("prepare");
        let original = [[0.0, 0.0], [0.25, -0.25], [0.5, -0.5], [0.75, -0.75], [0.0, 0.0]];
        write(&dir, "samples/a.wav", &original);
        let before = std::fs::read(dir.join("samples/a.wav")).unwrap();
        let result = prepare(&dir, "samples/a.wav", Edit { start: 0.2, end: 0.8, reverse: true, gain_db: -6.020_599_913, ..edit() }).unwrap();
        let channels = niminal_lang::Samples::new(&dir).load(&result).unwrap();
        assert_eq!(channels.len(), 2); assert_eq!(channels[0].len(), 3);
        assert!((channels[0].get(0) - 0.375).abs() < 1e-6);
        assert!((channels[1].get(2) + 0.125).abs() < 1e-6);
        assert_eq!(std::fs::read(dir.join("samples/a.wav")).unwrap(), before);
        let faded = prepare(&dir, "samples/a.wav", Edit { fade_in_ms: 1.0, fade_out_ms: 1.0, ..edit() }).unwrap();
        let channels = niminal_lang::Samples::new(&dir).load(&faded).unwrap();
        assert_eq!(channels[0].get(0), 0.0); assert_eq!(channels[0].get(4), 0.0);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn analysis_and_normalization_do_not_cancel_opposite_stereo_channels() {
        let dir = folder("analysis");
        write(&dir, "samples/a.wav", &[[0.0, 0.0], [0.25, -0.25], [0.5, -0.5], [0.0, 0.0]]);
        let a = analyze(&dir, "samples/a.wav").unwrap();
        assert_eq!((a.frames, a.channels, a.peak), (4, 2, 0.5));
        assert_eq!((a.silence_start, a.silence_end), (0.25, 0.75));
        let p = crate::slicing::peaks(&dir, "samples/a.wav", 2).unwrap();
        assert!(p.peaks.iter().any(|v| v.abs() == 0.5));
        let normalized = prepare(&dir, "samples/a.wav", Edit { normalize: true, ..edit() }).unwrap();
        assert!((analyze(&dir, &normalized).unwrap().peak - 0.891_250_9).abs() < 1e-6);
        write(&dir, "samples/tiny.wav", &[[1e-40, -1e-40]]);
        let normalized = prepare(&dir, "samples/tiny.wav", Edit { normalize: true, ..edit() }).unwrap();
        assert!((analyze(&dir, &normalized).unwrap().peak - 0.891_250_9).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn invalid_edits_are_rejected_and_nonfinite_audio_is_silenced() {
        let dir = folder("invalid"); write(&dir, "samples/a.wav", &[[f32::NAN, f32::INFINITY], [0.25, -0.25]]);
        for settings in [Edit { start: 0.5, end: 0.5, ..edit() }, Edit { gain_db: f64::NAN, ..edit() }, Edit { fade_out_ms: -1.0, ..edit() }] {
            assert!(prepare(&dir, "samples/a.wav", settings).is_err());
        }
        assert!(prepare(&dir, "../outside.wav", edit()).is_err());
        let rendered = prepare(&dir, "samples/a.wav", edit()).unwrap();
        let channels = niminal_lang::Samples::new(&dir).load(&rendered).unwrap();
        for channel in channels { assert_eq!(channel.get(0), 0.0); assert!(channel.get(1).is_finite()); }
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn replacing_a_kit_member_keeps_names_order_and_old_audio_and_finds_duplicates() {
        let dir = folder("kit"); std::fs::create_dir(dir.join("samples/kit")).unwrap();
        write(&dir, "samples/kit/closed.wav", &[[0.1, 0.2]; 16]);
        write(&dir, "samples/kit/open.wav", &[[0.3, 0.4]; 16]);
        write(&dir, "samples/new.wav", &[[0.5, 0.6]; 16]);
        let folder = replace_kit_member(&dir, "samples/kit", "closed", "samples/new.wav").unwrap();
        assert_eq!(kit_members(&dir, &folder).unwrap(), ["closed", "open"]);
        assert_eq!(analyze(&dir, "samples/kit/closed.wav").unwrap().peak, 0.2);
        assert_eq!(analyze(&dir, &format!("{folder}/closed.wav")).unwrap().peak, 0.6);
        assert!(assets(&dir).iter().any(|a| a.duplicate_of.is_some()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
