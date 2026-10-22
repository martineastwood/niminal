use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use niminal_ws::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::pattern::{Mode, Playback, Song, Track, pitch};
use crate::{
    mixer,
    sound::{Kind, Sound},
};

#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub connected: bool,
    pub playing: bool,
    pub row: Option<usize>,
    pub pattern_id: Option<String>,
    pub sequence_position: Option<usize>,
    pub sample_rate: f64,
    pub skipped_rows: u64,
    pub late_events: u64,
    pub error: Option<String>,
    pub instruments: BTreeMap<String, String>,
    pub effects: BTreeMap<String, String>,
    pub effect_parameters: crate::commands::EffectParameters,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    Connect {
        port: u16,
        token: Option<String>,
    },
    Disconnect,
    Apply {
        id: String,
        source: String,
        kind: Kind,
    },
    Play {
        song: Song,
        mode: Mode,
        pattern_id: String,
        start_position: usize,
    },
    Update {
        song: Song,
    },
    Audition {
        instrument: String,
        note: String,
        track: usize,
    },
    Stop,
}

type Request = (Command, mpsc::Sender<Result<Status, String>>);

pub struct Audio {
    sender: mpsc::Sender<Request>,
    status: Arc<Mutex<Status>>,
}

impl Audio {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel::<Request>();
        let status = Arc::new(Mutex::new(Status::default()));
        let shared = status.clone();
        thread::Builder::new()
            .name("tracker-playback".into())
            .spawn(move || {
                let mut worker = Worker::default();
                loop {
                    match receiver.recv_timeout(Duration::from_millis(25)) {
                        Ok((command, reply)) => {
                            let result = worker.command(command);
                            if let Err(error) = &result {
                                worker.status.error = Some(error.clone());
                            }
                            *shared.lock().unwrap_or_else(|e| e.into_inner()) =
                                worker.status.clone();
                            let _ = reply.send(result.map(|()| worker.status.clone()));
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    if worker.playback.is_some() {
                        if let Err(error) = worker.tick() {
                            let _ = worker.stop();
                            worker.status.error = Some(error);
                        }
                        *shared.lock().unwrap_or_else(|e| e.into_inner()) = worker.status.clone();
                    }
                }
                let _ = worker.stop();
            })
            .expect("could not start tracker playback worker");
        Self { sender, status }
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[derive(Default)]
struct Worker {
    client: Option<Client>,
    status: Status,
    sounds: BTreeMap<String, Sound>,
    mixer: Vec<Track>,
    playback: Option<Playback>,
    mixer_timing: (f64, u32),
}

impl Worker {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let result = self
            .client
            .as_mut()
            .ok_or("Connect to niminal first")?
            .call_raw(method, params);
        match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                let mut message = error["message"]
                    .as_str()
                    .unwrap_or("Engine rejected the request")
                    .to_string();
                if let Some(problems) = error["data"]["problems"].as_array() {
                    for problem in problems {
                        if let Some(text) = problem["message"].as_str() {
                            message.push_str(&format!("\n{text}"));
                        }
                    }
                }
                Err(message)
            }
            Err(error) => {
                self.client = None;
                self.playback = None;
                self.sounds.clear();
                self.mixer.clear();
                self.status = Status {
                    error: Some(error.clone()),
                    ..Status::default()
                };
                Err(format!("Engine disconnected: {error}"))
            }
        }
    }

    fn send_events(&mut self, events: Vec<Value>) -> Result<(), String> {
        if events.is_empty() {
            return Ok(());
        }
        let reply = self.call("events.send", json!({"events": events}))?;
        self.status.late_events += reply["late"].as_u64().unwrap_or(0);
        Ok(())
    }

    fn clock(&mut self) -> Result<u64, String> {
        self.call("clock", json!({}))?["sample"]
            .as_u64()
            .ok_or("Engine returned an invalid sample clock".into())
    }

    fn stop(&mut self) -> Result<(), String> {
        self.playback = None;
        self.status.playing = false;
        self.status.row = None;
        self.status.pattern_id = None;
        self.status.sequence_position = None;
        if self.client.is_some() {
            // Panic does not cancel future events, so cancel the queue first.
            self.call(
                "events.cancel",
                json!({"tag":"tracker-pattern", "from":"now"}),
            )?;
            self.call("panic", json!({}))?;
            self.send_events(mixer::reset(&self.mixer, &self.status.effect_parameters))?;
        }
        Ok(())
    }

    fn command(&mut self, command: Command) -> Result<(), String> {
        self.status.error = None;
        match command {
            Command::Connect { port, token } => {
                self.stop()?;
                self.client = None;
                self.sounds.clear();
                self.mixer.clear();
                self.status = Status::default();
                let (client, hello) =
                    Client::connect_as(port, token.as_deref(), "niminal-tracker")?;
                self.status.sample_rate = hello["sample_rate"]
                    .as_f64()
                    .filter(|rate| *rate > 0.0)
                    .ok_or("Invalid engine sample rate")?;
                self.client = Some(client);
                self.send_events(vec![
                    json!({"op":"routing", "at":"now", "edits":mixer::setup()}),
                ])?;
                self.status.connected = true;
            }
            Command::Disconnect => {
                let result = self.stop();
                self.client = None;
                self.sounds.clear();
                self.mixer.clear();
                self.status = Status::default();
                result?;
            }
            Command::Apply { id, source, kind } => {
                let sound = Sound::parse(kind, source)?;
                if self
                    .sounds
                    .iter()
                    .any(|(other, s)| other != &id && s.target == sound.target)
                {
                    return Err(format!(
                        "The name {} is already used by another slot",
                        sound.target
                    ));
                }
                let mut sounds = self.sounds.clone();
                sounds.insert(id, sound);
                let source = sounds
                    .values()
                    .map(|s| s.engine_source.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let instruments = sounds
                    .iter()
                    .filter(|(_, s)| s.fx_source.is_none())
                    .map(|(id, s)| (id.clone(), s.target.clone()))
                    .collect();
                let effects: BTreeMap<_, _> = sounds
                    .iter()
                    .filter(|(_, s)| s.fx_source.is_some())
                    .map(|(id, s)| (id.clone(), s.target.clone()))
                    .collect();
                let mut edits: Vec<_> = sounds
                    .values()
                    .filter_map(|s| {
                        s.fx_source
                            .as_ref()
                            .map(|body| json!({"op":"fx", "name":s.target, "source":body}))
                    })
                    .collect();
                let parameters: crate::commands::EffectParameters = sounds
                    .iter()
                    .filter(|(_, sound)| sound.fx_source.is_some())
                    .map(|(id, sound)| (id.clone(), sound.parameters.clone()))
                    .collect();
                for track in &self.mixer {
                    for slot in &track.effects {
                        for name in slot.modulations.keys() {
                            if !parameters.get(&slot.definition).is_some_and(|params| {
                                params.iter().any(|parameter| parameter.name == *name)
                            }) {
                                return Err(format!(
                                    "Remove the LFO targeting {name} before removing that effect parameter"
                                ));
                            }
                        }
                    }
                }
                edits.extend(mixer::replacements(&self.mixer, &effects, &parameters)?);
                let (_, modulators) = crate::modulation::edits(
                    &self.mixer,
                    &self.mixer,
                    &parameters,
                    self.mixer_timing.0,
                    self.mixer_timing.1,
                    true,
                );
                edits.extend(modulators);
                self.send_events(vec![
                    json!({"op":"transaction", "at":"now", "source":source, "edits":edits}),
                ])?;
                self.sounds = sounds;
                self.status.instruments = instruments;
                self.status.effects = effects;
                self.status.effect_parameters = parameters;
                if let Some(playback) = &mut self.playback {
                    playback.parameters = self.status.effect_parameters.clone();
                }
            }
            Command::Play {
                song,
                mode,
                pattern_id,
                start_position,
            } => {
                self.check_assignments(&song)?;
                let mut playback = Playback::new(0, song, mode, pattern_id, start_position)?;
                self.stop()?;
                self.set_mixer(&playback.song)?;
                self.send_events(mixer::reset(
                    &playback.song.tracks,
                    &self.status.effect_parameters,
                ))?;
                playback.parameters = self.status.effect_parameters.clone();
                playback.origin = self.clock()? + (self.status.sample_rate * 0.15).round() as u64;
                self.send_events(crate::modulation::reset(
                    &playback.song.tracks,
                    &self.status.effect_parameters,
                    playback.origin,
                ))?;
                self.status.skipped_rows = 0;
                self.status.late_events = 0;
                self.playback = Some(playback);
                self.status.playing = true;
                self.tick()?;
            }
            Command::Update { song } => {
                song.validate()?;
                self.check_assignments(&song)?;
                if let Some(playback) = &self.playback
                    && !playback.song.same_timeline(&song)
                {
                    return Err(
                        "Stop playback before changing the sequence, pattern lengths or tempo"
                            .into(),
                    );
                }
                self.set_mixer(&song)?;
                if let Some(playback) = &mut self.playback {
                    playback.song = song;
                    playback.parameters = self.status.effect_parameters.clone();
                }
            }
            Command::Audition {
                instrument,
                note,
                track,
            } => {
                if track >= 16 {
                    return Err("Invalid audition track".into());
                }
                let target = self
                    .status
                    .instruments
                    .get(&instrument)
                    .ok_or("Apply this instrument before auditioning")?
                    .clone();
                self.send_events(vec![json!({"op":"note", "at":"now", "target":target, "to":format!("track_{track}"), "dur":"200ms", "args":{"freq":pitch(&note)?}})])?;
            }
            Command::Stop => self.stop()?,
        }
        Ok(())
    }

    fn check_assignments(&self, song: &Song) -> Result<(), String> {
        song.validate()?;
        for (index, track) in song.tracks.iter().enumerate() {
            if let Some(id) = &track.instrument
                && !self.status.instruments.contains_key(id)
            {
                return Err(format!(
                    "Apply the instrument assigned to {} first",
                    track.name
                ));
            }
            for slot in &track.effects {
                let parameters = self
                    .status
                    .effect_parameters
                    .get(&slot.definition)
                    .ok_or_else(|| format!("Apply the effect assigned to {} first", track.name))?;
                for name in slot.modulations.keys() {
                    if !parameters.iter().any(|parameter| parameter.name == *name) {
                        return Err(format!("{}: unknown modulation target {name}", track.name));
                    }
                }
            }
            for pattern in &song.patterns {
                for lane in &pattern.fx[index] {
                    for value in lane.values.iter().flatten() {
                        let target = lane.target.as_ref().ok_or("Choose a lane parameter")?;
                        crate::commands::edit(
                            target,
                            value,
                            index,
                            track,
                            &self.status.effect_parameters,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    fn set_mixer(&mut self, song: &Song) -> Result<(), String> {
        let events = mixer::events(
            &song.tracks,
            &self.mixer,
            &self.status.effects,
            &self.status.effect_parameters,
            song.bpm,
            song.rows_per_beat,
            self.mixer_timing != (song.bpm, song.rows_per_beat),
        )?;
        self.send_events(events)?;
        self.mixer = song.tracks.clone();
        self.mixer_timing = (song.bpm, song.rows_per_beat);
        Ok(())
    }

    fn tick(&mut self) -> Result<(), String> {
        let sample = self.clock()?;
        let sr = self.status.sample_rate;
        if let Some(playback) = &mut self.playback {
            let position = playback.position(sample, sr);
            self.status.row = position.as_ref().map(|p| p.row);
            self.status.pattern_id = position.as_ref().map(|p| p.pattern.id.clone());
            self.status.sequence_position = position.as_ref().and_then(|p| p.sequence_position);
            let (events, skipped) = playback.events_until(
                sample + (sr * 0.02) as u64,
                sample + (sr * 0.2) as u64,
                sr,
                &self.status.instruments,
            )?;
            self.status.skipped_rows += skipped;
            let finished = playback.finished(sample, sr);
            self.send_events(events)?;
            if finished {
                // Natural song completion lets the final notes and effects ring out.
                self.playback = None;
                self.status.playing = false;
            }
        }
        Ok(())
    }
}

#[tauri::command]
pub async fn audio_command(
    command: Command,
    audio: tauri::State<'_, Audio>,
) -> Result<Status, String> {
    let sender = audio.sender.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (reply, response) = mpsc::channel();
        sender.send((command, reply)).map_err(|e| e.to_string())?;
        response.recv().map_err(|e| e.to_string())?
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn audio_status(audio: tauri::State<'_, Audio>) -> Status {
    audio.status()
}

#[cfg(test)]
#[path = "audio_tests.rs"]
mod tests;
