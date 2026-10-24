//! WAV decoding and seeking never run on the renderer.
use niminal_engine::{
    GraphBuilder, Src,
    ops::{STREAM_PAGE, StreamCache, StreamOut},
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, atomic::Ordering};
fn one() -> f64 {
    1.
}
fn yes() -> bool {
    true
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Player {
    pub path: String,
    #[serde(default)]
    pub channels: Vec<usize>,
    #[serde(default = "one")]
    pub rate: f64,
    #[serde(default)]
    pub position: f64,
    #[serde(default = "yes")]
    pub playing: bool,
    #[serde(default)]
    pub looping: bool,
    #[serde(default)]
    pub loop_start: f64,
    #[serde(default)]
    pub loop_end: Option<f64>,
}
pub(crate) fn prepare(
    player: &Player,
    prefix: &str,
) -> Result<(Arc<niminal_engine::Graph>, Vec<niminal_lang::ControlDef>), String> {
    if !player.rate.is_finite()
        || !(0.25..=4.).contains(&player.rate)
        || !player.position.is_finite()
        || player.position < 0.
    {
        return Err("player rate is 0.25–4 and position is nonnegative seconds".into());
    }
    let path = std::path::PathBuf::from(&player.path);
    let original = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    let fingerprint = (original.len(), original.modified().ok());
    let mut reader = hound::WavReader::open(&player.path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let frames = reader.duration() as u64;
    let channels = if player.channels.is_empty() {
        (1..=spec.channels as usize).collect::<Vec<_>>()
    } else {
        player.channels.clone()
    };
    if channels.is_empty()
        || channels.len() > 16
        || channels
            .iter()
            .any(|c| *c == 0 || *c > spec.channels as usize)
    {
        return Err("player channel selection is outside the file layout".into());
    }
    let end = player
        .loop_end
        .unwrap_or(frames as f64 / f64::from(spec.sample_rate));
    if !end.is_finite()
        || !player.loop_start.is_finite()
        || player.loop_start < 0.
        || end <= player.loop_start
        || end > frames as f64 / f64::from(spec.sample_rate)
    {
        return Err("player loop bounds are outside the file".into());
    }
    let cache = StreamCache::new(frames, channels.len(), spec.sample_rate);
    let start = (player.position * f64::from(spec.sample_rate)) as u64;
    cache.requested.store(start, Ordering::Relaxed);
    let first = start / STREAM_PAGE as u64;
    for page in first..first + 6 {
        fill(&mut reader, &cache, page, &channels)?;
    }
    let worker = cache.clone();
    std::thread::Builder::new()
        .name("niminal-stream".into())
        .spawn(move || {
            while Arc::strong_count(&worker) > 1 {
                if std::fs::metadata(&path)
                    .map(|m| (m.len(), m.modified().ok()))
                    .ok()
                    != Some(fingerprint)
                {
                    worker.failed.store(true, Ordering::Release);
                    return;
                }
                let page = worker.requested.load(Ordering::Acquire) / STREAM_PAGE as u64;
                let loop_start = worker.loop_start.load(Ordering::Relaxed);
                let mut wanted = (page..page + 6).collect::<Vec<_>>();
                if loop_start != u64::MAX {
                    wanted.push(loop_start / STREAM_PAGE as u64);
                }
                for page in wanted {
                    if !worker.cached(page) && fill(&mut reader, &worker, page, &channels).is_err()
                    {
                        worker.failed.store(true, Ordering::Release);
                        return;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        })
        .map_err(|e| e.to_string())?;
    let mut graph = GraphBuilder::new();
    let values = [
        ("rate", player.rate, niminal_lang::Unit::Num),
        (
            "playing",
            if player.playing { 1. } else { 0. },
            niminal_lang::Unit::Num,
        ),
        ("seek", player.position, niminal_lang::Unit::Time),
        ("seek_trigger", 0., niminal_lang::Unit::Num),
        (
            "loop",
            if player.looping { 1. } else { 0. },
            niminal_lang::Unit::Num,
        ),
        ("loop_start", player.loop_start, niminal_lang::Unit::Time),
        ("loop_end", end, niminal_lang::Unit::Time),
    ];
    let mut controls = Vec::new();
    let mut ports = Vec::<(&str, Src)>::new();
    for (name, value, unit) in values {
        let key = format!("{prefix}::{name}");
        let src = graph.param(&key, value as f32).unwrap();
        controls.push(niminal_lang::ControlDef {
            range: if name == "rate" {
                Some((0.25, 4.))
            } else {
                None
            },
            name: key.clone(),
            key,
            unit,
            initial: value as f32,
            smooth_seconds: 0.,
        });
        ports.push((name, src));
    }
    let output = graph
        .add_outputs(StreamOut::new(cache), &ports)
        .map_err(|e| e.to_string())?;
    Ok((Arc::new(graph.build_channels(&output)), controls))
}
fn fill(
    reader: &mut hound::WavReader<std::io::BufReader<std::fs::File>>,
    cache: &StreamCache,
    page: u64,
    channels: &[usize],
) -> Result<(), String> {
    let frame = page * STREAM_PAGE as u64;
    if frame >= cache.frames {
        cache.publish(page, &[]);
        return Ok(());
    }
    reader.seek(frame as u32).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let count = STREAM_PAGE.min((cache.frames - frame) as usize) * spec.channels as usize;
    let samples = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .take(count)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let scale = 2f64.powi(spec.bits_per_sample as i32 - 1);
            reader
                .samples::<i32>()
                .take(count)
                .map(|v| v.map(|v| (f64::from(v) / scale) as f32))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        }
    };
    let values: Vec<_> = samples
        .chunks(spec.channels as usize)
        .flat_map(|frame| channels.iter().map(move |c| frame[c - 1]))
        .collect();
    cache.publish(page, &values);
    Ok(())
}
