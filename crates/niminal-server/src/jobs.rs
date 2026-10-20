//! Independent offline sessions and bounded-memory capture workers.
use crate::{At, Endpoint, Op, Session};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
fn rate() -> u32 {
    48000
}
fn channels() -> usize {
    2
}
fn yes() -> bool {
    true
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderSpec {
    pub source: String,
    #[serde(default)]
    pub events: Vec<Op>,
    pub frames: u64,
    #[serde(default = "rate")]
    pub sample_rate: u32,
    #[serde(default = "channels")]
    pub channels: usize,
    #[serde(default = "yes")]
    pub limiter: bool,
    #[serde(default = "yes")]
    pub tail: bool,
    #[serde(default)]
    pub outputs: Vec<Output>,
    #[serde(default)]
    pub resources: Vec<Resource>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub id: String,
    pub endpoint: Endpoint,
    #[serde(default)]
    pub path: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub channel: usize,
}
type RenderedFile = (String, PathBuf, bool);
struct Progress {
    cancel: AtomicBool,
    frames: AtomicU64,
    result: Mutex<Option<Result<Vec<RenderedFile>, String>>>,
}
struct Job {
    progress: Arc<Progress>,
    state: Value,
    announced: Value,
    stop: Option<Arc<AtomicU64>>,
    from: u64,
}
#[derive(Default)]
pub(crate) struct Jobs {
    entries: BTreeMap<(u64, u64), Job>,
    next: u64,
}
impl Jobs {
    fn trim(&mut self, client: u64) {
        let completed = self
            .entries
            .iter()
            .filter(|((c, _), j)| *c == client && j.state["state"] != "running")
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        for key in completed.iter().take(completed.len().saturating_sub(63)) {
            self.entries.remove(key);
        }
    }
    pub fn render(
        &mut self,
        client: u64,
        mut spec: RenderSpec,
        files: &crate::files::Files,
    ) -> Result<Value, String> {
        if spec.frames == 0
            || !(8000..=384000).contains(&spec.sample_rate)
            || !(1..=16).contains(&spec.channels)
        {
            return Err("render requires positive frames, 8–384 kHz and 1–16 channels".into());
        }
        if self
            .entries
            .iter()
            .filter(|((c, _), j)| *c == client && j.state["state"] == "running")
            .count()
            >= 2
            || self
                .entries
                .values()
                .filter(|j| j.state["state"] == "running")
                .count()
                >= 4
        {
            return Err("render worker capacity reached".into());
        }
        for resource in &mut spec.resources {
            if resource.path.starts_with("file:") {
                resource.path = files
                    .path(client, &resource.path)?
                    .to_string_lossy()
                    .into_owned();
            }
        }
        self.trim(client);
        self.next += 1;
        let id = self.next;
        let progress = Arc::new(Progress {
            cancel: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            result: Mutex::new(None),
        });
        self.entries.insert(
            (client, id),
            Job {
                progress: progress.clone(),
                state: json!({"id":id,"state":"running","frames":0}),
                announced: Value::Null,
                stop: None,
                from: 0,
            },
        );
        std::thread::Builder::new()
            .name("niminal-export".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    render(spec, &progress)
                }))
                .unwrap_or_else(|_| Err("render worker panicked".into()));
                *progress.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            })
            .map_err(|e| {
                self.entries.remove(&(client, id));
                e.to_string()
            })?;
        Ok(json!({"id":id,"state":"running"}))
    }
    pub fn capture(
        &mut self,
        session: &Session,
        client: u64,
        output: Output,
        at: At,
    ) -> Result<Value, String> {
        let from = match at {
            At::Now => session.clock(),
            At::Sample(n) if n >= session.clock() => n,
            _ => return Err("capture starts in the future or now".into()),
        };
        if self
            .entries
            .iter()
            .filter(|((c, _), j)| *c == client && j.state["state"] == "running")
            .count()
            >= 2
        {
            return Err("at most two active jobs per client".into());
        }
        let meter = session.endpoint_meter(client, &output.endpoint)?;
        let heartbeat = session.endpoint_meter(client, &Endpoint::Master)?;
        heartbeat
            .waveform()
            .consumers
            .fetch_add(1, Ordering::Relaxed);
        meter.waveform().consumers.fetch_add(1, Ordering::Relaxed);
        self.trim(client);
        self.next += 1;
        let id = self.next;
        let rate = session.sample_rate() as u32;
        let progress = Arc::new(Progress {
            cancel: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            result: Mutex::new(None),
        });
        let stop = Arc::new(AtomicU64::new(u64::MAX));
        self.entries.insert(
            (client, id),
            Job {
                progress: progress.clone(),
                state: json!({"id":id,"state":"running","from":from}),
                announced: Value::Null,
                stop: Some(stop.clone()),
                from,
            },
        );
        std::thread::spawn(move || {
            let temporary = output.path.is_none();
            let path = output.path.map(PathBuf::from).unwrap_or_else(|| {
                std::env::temp_dir()
                    .join(format!("niminal-capture-{}-{id}.wav", std::process::id()))
            });
            let result = (|| {
                let mut writer = None::<crate::wav::Wav>;
                let mut next = from;
                loop {
                    if progress.cancel.load(Ordering::Acquire) {
                        if let Some(writer) = writer.take() {
                            writer.finish(None)?;
                        }
                        return Err("capture cancelled".into());
                    }
                    let end = stop.load(Ordering::Acquire);
                    if next >= end {
                        break;
                    }
                    if let Some((sample, width, values)) = meter.waveform().read(4096) {
                        let available = sample + (values.len() / width) as u64;
                        if available > next {
                            if sample > next {
                                return Err(format!(
                                    "capture overflow: {} frames missing",
                                    sample - next
                                ));
                            }
                            let to = available.min(end);
                            if writer.is_none() {
                                writer = Some(crate::wav::Wav::create(&path, width, rate)?);
                            }
                            writer.as_mut().unwrap().write(
                                &values[(next - sample) as usize * width
                                    ..(to - sample) as usize * width],
                            )?;
                            next = to;
                            progress.frames.store(next - from, Ordering::Release);
                        }
                    }
                    if heartbeat.waveform().end_sample() > next.saturating_add(4096) {
                        return Err("capture source stopped producing audio".into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                if let Some(writer) = writer {
                    writer.finish(None)?;
                } else {
                    return Err("capture ended before any audio was rendered".into());
                }
                Ok(vec![(output.id, path.clone(), temporary)])
            })();
            heartbeat
                .waveform()
                .consumers
                .fetch_sub(1, Ordering::Relaxed);
            meter.waveform().consumers.fetch_sub(1, Ordering::Relaxed);
            if result.is_err() && temporary {
                let _ = std::fs::remove_file(&path);
            }
            *progress.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        });
        Ok(json!({"id":id,"from":from,"state":"running"}))
    }
    pub fn stop_capture(&mut self, client: u64, id: u64, at: u64) -> Result<Value, String> {
        let job = self.entries.get(&(client, id)).ok_or("no such capture")?;
        let stop = job.stop.as_ref().ok_or("job is not a live capture")?;
        if at < job.from + job.progress.frames.load(Ordering::Acquire) {
            return Err("capture stop precedes samples already written".into());
        }
        stop.store(at, Ordering::Release);
        Ok(json!({"id":id,"stop":at}))
    }
    pub fn cancel(&mut self, client: u64, id: u64) -> Result<Value, String> {
        let job = self.entries.get(&(client, id)).ok_or("no such job")?;
        job.progress.cancel.store(true, Ordering::Release);
        Ok(json!({"id":id,"cancelling":true}))
    }
    pub fn status(
        &mut self,
        client: u64,
        id: u64,
        files: &mut crate::files::Files,
    ) -> Result<Value, String> {
        self.poll(files);
        self.entries
            .get(&(client, id))
            .map(|j| j.state.clone())
            .ok_or_else(|| "no such job".into())
    }
    pub fn poll(&mut self, files: &mut crate::files::Files) -> Vec<(u64, Value)> {
        let mut updates = Vec::new();
        for ((client, id), job) in &mut self.entries {
            if let Some(result) = job
                .progress
                .result
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                match result {
                    Ok(outputs) => {
                        let outputs:Vec<_>=outputs.into_iter().map(|(name,path,temporary)|{let handle=files.register(*client,path.clone(),temporary);json!({"id":name,"file":handle,"path":(!temporary).then(||path.to_string_lossy().into_owned())})}).collect();
                        job.state = json!({"id":id,"state":"completed","outputs":outputs});
                    }
                    Err(error) => {
                        job.state = json!({"id":id,"state":if job.progress.cancel.load(Ordering::Acquire){"cancelled"}else{"failed"},"error":error});
                    }
                }
            }
            job.state["frames"] = json!(job.progress.frames.load(Ordering::Acquire));
            if job.state != job.announced {
                job.announced = job.state.clone();
                updates.push((*client, job.state.clone()));
            }
        }
        updates
    }
    pub fn disconnect(&mut self, client: u64) {
        for ((c, _), j) in &self.entries {
            if *c == client {
                j.progress.cancel.store(true, Ordering::Release);
            }
        }
        self.entries.retain(|(c, _), _| *c != client);
    }
}
fn render(
    mut spec: RenderSpec,
    progress: &Progress,
) -> Result<Vec<(String, PathBuf, bool)>, String> {
    let mut session = Session::new(
        spec.sample_rate as f32,
        match spec.channels {
            4 => niminal_lang::Layout::Quad,
            6 => niminal_lang::Layout::Surround51,
            8 => niminal_lang::Layout::Surround71,
            n => niminal_lang::Layout::from_count(n),
        },
    );
    if !spec.limiter {
        session = session.without_limiter();
    }
    for resource in spec.resources {
        crate::resources::request(
            &mut session,
            0,
            crate::EngineRequest::BuffersLoad {
                name: resource.name,
                path: resource.path,
                channel: resource.channel,
            },
        )
        .map_err(|e| e.to_string())?;
    }
    session
        .send_events(
            0,
            vec![Op::Define {
                source: spec.source,
                at: At::Now,
                dir: None,
                tag: None,
            }],
        )
        .map_err(|p| p[0].message.clone())?;
    for event in &spec.events {
        let at = match serde_json::to_value(event).map_err(|e| e.to_string())?["at"].clone() {
            Value::Number(n) => n.as_u64().ok_or("invalid event sample")?,
            _ => 0,
        };
        if at >= spec.frames {
            return Err("render events must precede the requested end".into());
        }
    }
    let mut events = spec
        .events
        .into_iter()
        .map(|event| {
            let value = serde_json::to_value(&event).expect("event");
            let at = value["at"].as_u64().unwrap_or(0);
            (at, event)
        })
        .collect::<Vec<_>>();
    events.sort_by_key(|(at, _)| *at);
    let mut events = std::collections::VecDeque::from(events);
    let mut initial = Vec::new();
    while events.front().is_some_and(|(at, _)| *at == 0) {
        initial.push(events.pop_front().unwrap().1);
    }
    if !initial.is_empty() {
        session
            .send_events(0, initial)
            .map_err(|p| p[0].message.clone())?;
    }
    if spec.outputs.is_empty() {
        spec.outputs.push(Output {
            id: "master".into(),
            endpoint: Endpoint::Master,
            path: None,
        });
    }
    let mut outputs = Vec::new();
    for (index, output) in spec.outputs.into_iter().enumerate() {
        let meter = session.endpoint_meter(0, &output.endpoint)?;
        meter.waveform().enabled.store(true, Ordering::Relaxed);
        let temporary = output.path.is_none();
        let path = output.path.map(PathBuf::from).unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "niminal-export-{}-{}-{index}.wav",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ))
        });
        outputs.push((output.id, meter, path, temporary, None::<crate::wav::Wav>));
    }
    let result = (|| {
        let mut last_sound = spec.frames;
        let mut stop = spec.frames;
        let maximum = spec.frames
            + if spec.tail {
                spec.sample_rate as u64 * 60
            } else {
                0
            };
        let mut released = false;
        while session.clock() < maximum {
            if progress.cancel.load(Ordering::Acquire) {
                return Err("render cancelled".into());
            }
            if session.clock() >= spec.frames && !released {
                session.cancel_events(0, None, At::Now, true);
                released = true;
            }
            let n =
                32.min((if released { maximum } else { spec.frames }) - session.clock()) as usize;
            let mut due = Vec::new();
            while events
                .front()
                .is_some_and(|(at, _)| *at < session.clock() + n as u64)
            {
                due.push(events.pop_front().unwrap().1);
            }
            if !due.is_empty() {
                session
                    .send_events(0, due)
                    .map_err(|p| p[0].message.clone())?;
            }
            session.render(n, &mut |_, _| {});
            if session
                .program()
                .tracks
                .iter()
                .filter_map(|t| t.def.chain.as_ref())
                .filter_map(|g| g.stream_cache())
                .any(|c| c.failed.load(Ordering::Acquire))
            {
                return Err("render input stream failed or changed during playback".into());
            }
            for (_, meter, path, _, writer) in &mut outputs {
                let (sample, width, mut values) = meter
                    .waveform()
                    .read(n)
                    .ok_or("could not capture offline output")?;
                if sample != session.clock() - n as u64 {
                    values = vec![0.; n * width];
                }
                if writer.is_none() {
                    *writer = Some(crate::wav::Wav::create(path, width, spec.sample_rate)?);
                }
                writer.as_mut().unwrap().write(&values)?;
                for (i, frame) in values.chunks(width).enumerate() {
                    if frame.iter().any(|v| v.abs() > 3.1622777e-5) {
                        last_sound = last_sound.max(session.clock() - n as u64 + i as u64 + 1);
                    }
                }
            }
            progress.frames.store(session.clock(), Ordering::Release);
            stop = session.clock();
            if released && stop.saturating_sub(last_sound) >= spec.sample_rate as u64 {
                stop = last_sound;
                break;
            }
            // Lifecycle receipts are not exported, but must remain bounded for long projects.
            session.take_voice_updates();
        }
        for (_, _, _, _, writer) in &mut outputs {
            if let Some(writer) = writer.take() {
                writer.finish(Some(stop))?;
            }
        }
        Ok(outputs
            .iter()
            .map(|(id, _, path, temporary, _)| (id.clone(), path.clone(), *temporary))
            .collect())
    })();
    if result.is_err() {
        for (_, _, path, temporary, _) in &outputs {
            if *temporary {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    result
}

impl Drop for Progress {
    fn drop(&mut self) {
        if let Some(Ok(outputs)) = self
            .result
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            for (_, path, temporary) in outputs {
                if temporary {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
}
