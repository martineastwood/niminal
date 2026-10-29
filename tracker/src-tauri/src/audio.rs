use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use niminal_ws::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::pattern::{Mode, Playback, Song, Track, commands_of, fit_to_tempo, hit_args, note_args, retrigger_of, start_args, start_of};
use crate::{
    mixer,
    sound::{Kind, Sound},
};

#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub connected: bool,
    pub playing: bool,
    pub row: Option<usize>,
    /// Progress through `row` (0 to 1), measured at `row_at_ms` (Unix time), so the UI can predict row changes exactly.
    pub row_phase: Option<f64>,
    pub row_at_ms: f64,
    pub pattern_id: Option<String>,
    pub sequence_position: Option<usize>,
    pub sample_rate: f64,
    pub skipped_rows: u64,
    pub late_events: u64,
    pub error: Option<String>,
    pub instruments: BTreeMap<String, String>,
    pub effects: BTreeMap<String, String>,
    pub effect_parameters: crate::commands::EffectParameters,
    pub instrument_parameters: crate::commands::EffectParameters,
    /// The pieces of each sliced sample instrument, by id: its notes pick one, counting up from C-4.
    pub instrument_slices: BTreeMap<String, usize>,
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
        /// Where `sample("...")` paths in the source are resolved; kept for later applies.
        #[serde(default)]
        dir: Option<String>,
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
    /// Render the whole song, from its first step, to a WAV file; with `stems`, each track and return
    /// also gets a file beside it.
    Export {
        song: Song,
        path: String,
        #[serde(default)]
        stems: bool,
    },
    /// A MIDI key owns one held preview, independent of pattern playback.
    MidiOn { key: u16, instrument: String, note: String, track: usize, velocity: u8 },
    /// None releases all MIDI previews; a key releases only its own preview.
    MidiOff { key: Option<u16> },
    Audition {
        instrument: String,
        note: String,
        track: usize,
        /// How long the note is held, in milliseconds.
        #[serde(default = "audition_ms")]
        ms: u32,
        /// Note commands (`S20`, `R41`) to hear it with.
        #[serde(default)]
        commands: Vec<String>,
    },
    Stop,
    /// The tracker is closing: stop the sound and the engine it started.
    Quit,
    /// Whether the mixer view needs every strip's scope; the master's is always on.
    Scopes {
        strips: bool,
    },
}

fn audition_ms() -> u32 {
    300
}

type Request = (Command, mpsc::Sender<Result<Status, String>>);

/// Each channel's recent waveform, by scope id: `s_<track>` and `s_m` for the master.
pub type Scopes = BTreeMap<String, Vec<f32>>;
/// Frames per scope, and how often the engine sends them.
const SCOPE_FRAMES: usize = 128;
const SCOPE_RATE: f64 = 20.0;

pub struct Audio {
    sender: mpsc::Sender<Request>,
    status: Arc<Mutex<Status>>,
    scopes: Arc<Mutex<Scopes>>,
}

impl Audio {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel::<Request>();
        let status = Arc::new(Mutex::new(Status::default()));
        let shared = status.clone();
        let scopes = Arc::new(Mutex::new(Scopes::new()));
        let published = scopes.clone();
        thread::Builder::new()
            .name("tracker-playback".into())
            .spawn(move || {
                let mut worker = Worker::default();
                let mut last_scopes = std::time::Instant::now();
                let mut last_sound = std::time::Instant::now();
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
                    // after two silent seconds with nothing playing, ask the engine for scope frames less often
                    let wait = if worker.playback.is_none() && last_sound.elapsed() >= Duration::from_secs(2) { 150 } else { 30 };
                    if worker.client.is_some() && last_scopes.elapsed() >= Duration::from_millis(wait)
                    {
                        last_scopes = std::time::Instant::now();
                        if worker.pump().is_ok() {
                            if worker.scopes.values().any(|frames| frames.iter().any(|v| v.abs() >= 0.001)) {
                                last_sound = std::time::Instant::now();
                            }
                            *published.lock().unwrap_or_else(|e| e.into_inner()) =
                                worker.scopes.clone();
                        }
                        // A failed idle health check clears the connection too; publish it so
                        // the window can reconnect even when no pattern is playing.
                        *shared.lock().unwrap_or_else(|e| e.into_inner()) = worker.status.clone();
                    } else if worker.client.is_none() {
                        published.lock().unwrap_or_else(|e| e.into_inner()).clear();
                    }
                }
                let _ = worker.stop();
            })
            .expect("could not start tracker playback worker");
        Self {
            sender,
            status,
            scopes,
        }
    }

    /// Stops the sound and the engine this tracker started. Waits for the worker, so call it as the app closes.
    pub fn quit(&self) {
        let (reply, response) = mpsc::channel();
        if self.sender.send((Command::Quit, reply)).is_ok() {
            let _ = response.recv_timeout(Duration::from_secs(2));
        }
    }

    pub fn scopes(&self) -> Scopes {
        self.scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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
    mixer_returns: Vec<Track>,
    mixer_master: Track,
    midi_keys: BTreeMap<u16, (String, u64)>,
    instrument_values: BTreeMap<String, BTreeMap<String, f64>>,
    scopes: Scopes,
    strip_scopes: bool,
    /// The strips whose scopes the engine has made.
    scope_labels: BTreeSet<String>,
    playback: Option<Playback>,
    mixer_timing: (f64, u32),
    /// The bar's length in quarter-note beats, from the song's first pattern: what `1bar` in an effect is worked out at.
    bar_beats: f64,
    dir: Option<String>,
}

/// The bar's length in quarter-note beats, from the song's first pattern.
fn bar_beats_of(song: &Song) -> f64 {
    song.patterns.first().and_then(|p| p.timing.as_ref()).map_or(4.0, |t| f64::from(t.beats_per_bar) * 4.0 / f64::from(t.beat_unit))
}

impl Worker {
    /// Connects to the engine on `port`. The engine is the user's to run (`niminal server`).
    fn connect(&mut self, port: u16, token: Option<&str>) -> Result<(Client, Value), String> {
        Client::connect_as(port, token, "niminal-tracker")
    }

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
                self.instrument_values.clear();
                self.mixer.clear();
                self.mixer_returns.clear();
                self.mixer_master = Track::default();
                self.status = Status {
                    error: Some(error.clone()),
                    ..Status::default()
                };
                Err(format!("Engine disconnected: {error}"))
            }
        }
    }

    /// The engine's tempo and meter from `at`, as the song has them; `restart` counts the beats from 0 there.
    fn tempo_event(&self, at: Value, restart: bool) -> Value {
        let mut event = json!({"op":"tempo", "at":at, "bpm":self.mixer_timing.0, "meter":self.bar_beats});
        if restart {
            event["beat"] = json!(0.0);
        }
        event
    }

    fn send_events(&mut self, events: Vec<Value>) -> Result<(), String> {
        if events.is_empty() {
            return Ok(());
        }
        let reply = self.call("events.send", json!({"events": events}))?;
        self.status.late_events += reply["late"].as_u64().unwrap_or(0);
        Ok(())
    }

    /// Collects the scope frames the engine has sent. Notifications only arrive during a call, so ask
    /// for the clock to give them a chance.
    fn pump(&mut self) -> Result<(), String> {
        self.clock()?;
        let Some(client) = self.client.as_mut() else {
            return Ok(());
        };
        for note in client.take_notifications() {
            if note["method"] != "scope" {
                continue;
            }
            let params = &note["params"];
            let (Some(id), Some(values)) = (params["id"].as_str(), params["values"].as_array())
            else {
                continue;
            };
            // Channels are interleaved; one line per scope is enough, so average them.
            let channels = params["channels"].as_u64().unwrap_or(1).max(1) as usize;
            let frames = values
                .chunks(channels)
                .map(|frame| {
                    let mean =
                        frame.iter().filter_map(Value::as_f64).sum::<f64>() / channels as f64;
                    (mean * 1000.0).round() as f32 / 1000.0
                })
                .collect();
            self.scopes.insert(id.to_string(), frames);
        }
        Ok(())
    }

    /// Makes the engine's scopes match what is wanted: the master's, and with `strip_scopes` one for each
    /// track and return the song has. Each scope costs a JSON message every frame it reports.
    fn sync_scopes(&mut self) -> Result<(), String> {
        let mut wanted = BTreeSet::from([mixer::MASTER.to_string()]);
        if self.strip_scopes {
            wanted.extend(mixer::labels(&self.mixer, &self.mixer_returns));
        }
        for label in self.scope_labels.difference(&wanted).cloned().collect::<Vec<_>>() {
            // a strip that is gone took its tap with it, and the scope may have gone too
            let _ = self.call("scopes.remove", json!({"id":format!("s_{label}")}));
            self.scopes.remove(&format!("s_{label}"));
        }
        for label in wanted.difference(&self.scope_labels).cloned().collect::<Vec<_>>() {
            self.call(
                "scopes.create",
                json!({"id":format!("s_{label}"), "endpoint":{"kind":"node", "id":format!("tap_{label}")}, "frames":SCOPE_FRAMES, "rate":SCOPE_RATE}),
            )?;
        }
        self.scope_labels = wanted;
        Ok(())
    }

    fn clock(&mut self) -> Result<u64, String> {
        self.call("clock", json!({}))?["sample"]
            .as_u64()
            .ok_or("Engine returned an invalid sample clock".into())
    }

    fn release_midi(&mut self, key: Option<u16>) -> Result<(), String> {
        let keys: Vec<_> = self.midi_keys.keys().copied().filter(|k| key.is_none_or(|key| key == *k)).collect();
        for key in keys {
            if self.client.is_some() {
                self.call("events.cancel", json!({"tag":format!("tracker-midi-{key}"), "from":"now", "release":true}))?;
            }
            self.midi_keys.remove(&key);
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        self.release_midi(None)?;
        self.playback = None;
        self.status.playing = false;
        self.status.row = None;
        self.status.row_phase = None;
        self.status.pattern_id = None;
        self.status.sequence_position = None;
        if self.client.is_some() {
            // Panic does not cancel future events, so cancel the queue first.
            self.call(
                "events.cancel",
                json!({"tag":"tracker-pattern", "from":"now"}),
            )?;
            self.call("panic", json!({}))?;
            self.send_events(mixer::reset(
                &self.mixer,
                &self.mixer_returns,
                &self.mixer_master,
                &self.status.effect_parameters,
            ))?;
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
                self.mixer_returns.clear();
                self.mixer_master = Track::default();
                self.status = Status::default();
                let (client, hello) = self.connect(port, token.as_deref())?;
                self.status.sample_rate = hello["sample_rate"]
                    .as_f64()
                    .filter(|rate| *rate > 0.0)
                    .ok_or("Invalid engine sample rate")?;
                self.client = Some(client);
                self.send_events(vec![
                    json!({"op":"routing", "at":"now", "edits":mixer::setup()}),
                ])?;
                self.scopes.clear();
                self.scope_labels.clear();
                self.call("subscribe", json!({"topics":["scopes"]}))?;
                self.sync_scopes()?;
                self.status.connected = true;
            }
            Command::Disconnect => {
                let result = self.stop();
                self.client = None;
                self.sounds.clear();
                self.mixer.clear();
                self.mixer_returns.clear();
                self.mixer_master = Track::default();
                self.status = Status::default();
                result?;
            }
            Command::Apply {
                id,
                source,
                kind,
                dir,
            } => {
                let sound = Sound::parse(kind, source, dir.as_deref().or(self.dir.as_deref()), self.mixer_timing.0, self.bar_beats)?;
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
                let loop_args: serde_json::Map<String, Value> = sound.parameters.iter()
                    .filter(|parameter| matches!(parameter.name.as_str(), "loop_start" | "loop_end" | "loop_crossfade"))
                    .filter(|parameter| self.sounds.get(&id).is_some_and(|old| old.parameters.iter().any(|p| p.name == parameter.name && p.default != parameter.default)))
                    .map(|parameter| (parameter.name.clone(), parameter.argument(parameter.default))).collect();
                sounds.insert(id.clone(), sound);
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
                let (remove_modulators, _) = crate::modulation::edits(
                    &[],
                    &[],
                    &Track::default(),
                    &self.mixer,
                    &self.mixer_returns,
                    &self.mixer_master,
                    &self.status.effect_parameters,
                    self.mixer_timing.0,
                    self.mixer_timing.1,
                    false,
                );
                edits.extend(remove_modulators);
                edits.extend(mixer::replacements(
                    &self.mixer,
                    &self.mixer_returns,
                    &self.mixer_master,
                    &effects,
                    &parameters,
                )?);
                let (_, modulators) = crate::modulation::edits(
                    &self.mixer,
                    &self.mixer_returns,
                    &self.mixer_master,
                    &[],
                    &[],
                    &Track::default(),
                    &parameters,
                    self.mixer_timing.0,
                    self.mixer_timing.1,
                    true,
                );
                edits.extend(modulators);
                if dir.is_some() {
                    self.dir = dir;
                }
                // Effects travel as edits; with no instrument what is left to define is only comments.
                let defines = source.lines().any(|line| !line.trim().is_empty() && !line.trim_start().starts_with("//"));
                let mut transaction = if !defines {
                    json!({"op":"routing", "at":"now", "edits":edits})
                } else {
                    json!({"op":"transaction", "at":"now", "source":source, "edits":edits})
                };
                if let Some(dir) = &self.dir
                    && defines
                {
                    transaction["dir"] = json!(dir);
                }
                let mut events = vec![transaction];
                if !loop_args.is_empty() {
                    let args = Value::Object(loop_args);
                    let now = self.clock()?;
                    if let Some(playback) = &self.playback {
                        events.extend(playback.loop_edits(&id, &args, now));
                    }
                    events.extend(self.midi_keys.values().filter(|(instrument, _)| instrument == &id)
                        .map(|(_, voice)| json!({"op":"set", "id":voice, "at":"now", "args":args})));
                }
                self.send_events(events)?;
                self.sounds = sounds;
                self.status.instruments = instruments;
                self.status.effects = effects;
                self.status.effect_parameters = parameters;
                self.status.instrument_slices = self
                    .sounds
                    .iter()
                    .filter_map(|(id, sound)| Some((id.clone(), sound.slices?)))
                    .collect();
                self.status.instrument_parameters = self
                    .sounds
                    .iter()
                    .filter(|(_, sound)| sound.fx_source.is_none())
                    .map(|(id, sound)| (id.clone(), sound.parameters.clone()))
                    .collect();
                if let Some(playback) = &mut self.playback {
                    playback.parameters = self.status.effect_parameters.clone();
                    playback.instrument_parameters = self.status.instrument_parameters.clone();
                    playback.instrument_slices = self.status.instrument_slices.clone();
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
                    &playback.song.returns,
                    &playback.song.master,
                    &self.status.effect_parameters,
                ))?;
                playback.parameters = self.status.effect_parameters.clone();
                playback.instrument_parameters = self.status.instrument_parameters.clone();
                playback.instrument_slices = self.status.instrument_slices.clone();
                playback.origin = self.clock()? + (self.status.sample_rate * 0.15).round() as u64;
                self.send_events(vec![self.tempo_event(json!(playback.origin), true)])?;
                self.send_events(crate::modulation::reset(
                    &playback.song.tracks,
                    &playback.song.returns,
                    &playback.song.master,
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
                        "Stop playback before changing the sequence, pattern lengths"
                            .into(),
                    );
                }
                self.set_mixer(&song)?;
                let now = if self.playback.is_some() { self.clock()? } else { 0 };
                let sample_rate = self.status.sample_rate;
                if let Some(playback) = &mut self.playback {
                    if playback.song.loop_range != song.loop_range {
                        playback.set_loop(song.loop_range, now, sample_rate);
                    }
                    playback.update_song(song);
                    playback.parameters = self.status.effect_parameters.clone();
                    playback.instrument_parameters = self.status.instrument_parameters.clone();
                }
            }
            Command::Export { song, path, stems } => self.export(song, &path, stems)?,
            Command::MidiOn { key, instrument, note, track, velocity } => {
                if key >= 2048 || track >= self.mixer.len() || !(1..=127).contains(&velocity) {
                    return Err("Invalid MIDI note, track or velocity".into());
                }
                self.release_midi(Some(key))?;
                let target = self.status.instruments.get(&instrument).ok_or("Apply this instrument before playing MIDI")?.clone();
                let slices = self.status.instrument_slices.get(&instrument).copied();
                let Some(mut args) = note_args(&note, slices)? else { return Ok(()); };
                for parameter in self.status.instrument_parameters.get(&instrument).into_iter().flatten() {
                    if let Some(value) = self.instrument_values.get(&instrument).and_then(|v| v.get(&parameter.name)) {
                        args[&parameter.name] = parameter.argument(value.clamp(parameter.min, parameter.max));
                    }
                }
                fit_to_tempo(&mut args, self.status.instrument_parameters.get(&instrument).map_or(&[], Vec::as_slice), self.mixer_timing.0);
                args["gain"] = json!(format!("{}db", 20.0 * (f64::from(velocity) / 127.0).log10()));
                let id = crate::pattern::NOTE_IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.send_events(vec![json!({"op":"note", "at":"now", "id":id, "tag":format!("tracker-midi-{key}"), "target":target, "to":format!("in_{track}"), "args":args})])?;
                self.midi_keys.insert(key, (instrument, id));
            }
            Command::MidiOff { key } => self.release_midi(key)?,
            Command::Audition {
                instrument,
                note,
                track,
                ms,
                commands,
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
                let slices = self.status.instrument_slices.get(&instrument).copied();
                let Some(mut args) = note_args(&note, slices)? else {
                    return Ok(());
                };
                let commands = commands_of(commands.iter().map(String::as_str));
                let start = start_of(&commands);
                fit_to_tempo(&mut args, self.status.instrument_parameters.get(&instrument).map_or(&[], Vec::as_slice), self.mixer_timing.0);
                start_args(&mut args, start, self.status.instrument_parameters.get(&instrument).map_or(&[], Vec::as_slice));
                // a retrigger shares the time between its hits
                let retrigger = retrigger_of(&commands);
                let hits = retrigger.map_or(1, |r| r.hits);
                let start = if hits > 1 { self.clock()? + (self.status.sample_rate * 0.02) as u64 } else { 0 };
                let events = (0..hits)
                    .map(|hit| {
                        let at = if hits > 1 { json!(start + (f64::from(hit) * f64::from(ms) / f64::from(hits) / 1000.0 * self.status.sample_rate) as u64) } else { json!("now") };
                        json!({"op":"note", "at":at, "target":target, "to":format!("in_{track}"), "dur":format!("{}ms", ms / hits), "args":retrigger.map_or_else(|| args.clone(), |r| hit_args(&args, r.gain(hit), self.status.instrument_parameters.get(&instrument).map_or(&[], Vec::as_slice)))})
                    })
                    .collect();
                self.send_events(events)?;
            }
            Command::Stop => self.stop()?,
            Command::Quit => {
                let result = self.stop();
                self.client = None;
                return result;
            }
            Command::Scopes { strips } => {
                if self.strip_scopes != strips {
                    self.strip_scopes = strips;
                    if self.client.is_some() {
                        self.sync_scopes()?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether every value a parameter lane holds can be sent to the strip named `slot`.
    fn check_lane(&self, slot: &str, strip: &Track, lane: &crate::pattern::FxLane) -> Result<(), String> {
        let parameters = &self.status.effect_parameters;
        // an instrument's parameter is given to its notes as they sound, so there is no mixer edit to check
        if lane.target.as_ref().is_some_and(|target| !target.available(strip, parameters) || matches!(target, crate::commands::FxTarget::Instrument { .. })) {
            return Ok(());
        }
        if let Some(envelope) = &lane.envelope {
            for point in &envelope.points {
                let target = lane.target.as_ref().ok_or("Choose an envelope parameter")?;
                crate::commands::edit(target, &point.value.to_string(), slot, strip, parameters)?;
            }
        }
        Ok(())
    }

    fn check_assignments(&self, song: &Song) -> Result<(), String> {
        song.validate()?;
        for (strip, name) in song
            .returns
            .iter()
            .map(|r| (r, r.name.as_str()))
            .chain([(&song.master, "the master")])
        {
            if strip.fx.iter().any(|device| {
                !self
                    .status
                    .effect_parameters
                    .contains_key(&device.definition)
            }) {
                return Err(format!("Apply the effects assigned to {name} first"));
            }
        }
        // returns and the master have parameter lanes too
        for pattern in &song.patterns {
            for (r, lanes) in pattern.return_fx.iter().enumerate() {
                for lane in lanes {
                    self.check_lane(&format!("r{r}"), &song.returns[r], lane)?;
                }
            }
            for lane in &pattern.master_fx {
                self.check_lane(mixer::MASTER, &song.master, lane)?;
            }
        }
        for (index, track) in song.tracks.iter().enumerate() {
            if track.fx.iter().any(|device| {
                !self
                    .status
                    .effect_parameters
                    .contains_key(&device.definition)
            }) {
                return Err(format!(
                    "Apply the effects assigned to {} first",
                    track.name
                ));
            }
            for pattern in &song.patterns {
                for column in &pattern.notes[index] {
                    if let Some(id) = column
                        .inst
                        .iter()
                        .flatten()
                        .find(|id| !self.status.instruments.contains_key(*id))
                    {
                        return Err(format!(
                            "Apply the instrument {id} played on {} first",
                            track.name
                        ));
                    }
                }
                for lane in &pattern.fx[index] {
                    self.check_lane(&index.to_string(), track, lane)?;
                }
            }
        }
        Ok(())
    }

    fn set_mixer(&mut self, song: &Song) -> Result<(), String> {
        let events = mixer::events(
            &song.tracks,
            &song.returns,
            &song.master,
            &self.mixer,
            &self.mixer_returns,
            &self.mixer_master,
            &self.status.effects,
            &self.status.effect_parameters,
            song.bpm,
            song.rows_per_beat,
            self.mixer_timing != (song.bpm, song.rows_per_beat),
        )?;
        self.send_events(events)?;
        self.instrument_values = song.instrument_values.clone();
        self.mixer = song.tracks.clone();
        self.mixer_returns = song.returns.clone();
        self.mixer_master = song.master.clone();
        let (old_bpm, old_bar) = (self.mixer_timing.0, self.bar_beats);
        self.mixer_timing = (song.bpm, song.rows_per_beat);
        self.bar_beats = bar_beats_of(song);
        // what an effect reads as `clock.bpm` follows the song's tempo
        if (old_bpm, old_bar) != (song.bpm, self.bar_beats) {
            self.send_events(vec![self.tempo_event(json!("now"), false)])?;
        }
        // strips may have come or gone
        if self.strip_scopes && self.client.is_some() {
            self.sync_scopes()?;
        }
        Ok(())
    }

    /// Renders the song in an engine session of its own, faster than it plays, so the live sound is
    /// untouched. The session is built the way a fresh connection is: strips, definitions, mixer,
    /// then every note and lane edit of the song, all handed over in one request.
    fn export(&mut self, mut song: Song, path: &str, stems: bool) -> Result<(), String> {
        self.check_assignments(&song)?;
        self.stop()?;
        song.loop_range = None;
        let sample_rate = self.status.sample_rate;
        let origin = (sample_rate * 0.1) as u64;
        let first = song.sequence.first().ok_or("The song has no steps")?.clone();
        let mut playback = Playback::new(origin, song.clone(), Mode::Song, first, 0)?;
        playback.parameters = self.status.effect_parameters.clone();
        playback.instrument_parameters = self.status.instrument_parameters.clone();
        playback.instrument_slices = self.status.instrument_slices.clone();

        let source = self.sounds.values().map(|s| s.engine_source.as_str()).collect::<Vec<_>>().join("\n");
        let fx: Vec<_> = self
            .sounds
            .values()
            .filter_map(|s| s.fx_source.as_ref().map(|body| json!({"op":"fx", "name":s.target, "source":body})))
            .collect();
        let mut transaction = json!({"op":"transaction", "at":"now", "source":source, "edits":fx});
        if let Some(dir) = &self.dir {
            transaction["dir"] = json!(dir);
        }
        let mut events = vec![json!({"op":"routing", "at":"now", "edits":mixer::setup()}), transaction];
        events.push(json!({"op":"tempo", "at":origin, "bpm":song.bpm, "meter":bar_beats_of(&song), "beat":0.0}));
        events.extend(mixer::events(
            &song.tracks, &song.returns, &song.master, &[], &[], &Track::default(),
            &self.status.effects, &self.status.effect_parameters, song.bpm, song.rows_per_beat, true,
        )?);
        events.extend(mixer::reset(&song.tracks, &song.returns, &song.master, &self.status.effect_parameters));
        events.extend(crate::modulation::reset(
            &song.tracks, &song.returns, &song.master, &self.status.effect_parameters, origin,
        ));
        let rows: u64 = song
            .sequence
            .iter()
            .filter_map(|id| song.patterns.iter().find(|p| p.id == *id))
            .map(|p| p.rows as u64)
            .sum();
        let frames = playback.row_at(rows, sample_rate);
        let (notes, _) = playback.events_until(0, frames, sample_rate, &self.status.instruments)?;
        // the render takes only what happens before its end, so drop the closing edit of a held envelope
        events.extend(notes.into_iter().filter(|e| e["at"].as_u64().is_none_or(|at| at < frames)));

        let target = std::path::Path::new(path);
        let mut outputs = vec![json!({"id":"mix", "endpoint":{"kind":"master"}, "path":path})];
        if stems {
            let stem = target.with_extension("");
            let stem = stem.to_string_lossy();
            let strips = song.tracks.iter().enumerate().filter(|(i, _)| !mixer::silent(&song.tracks, *i)).map(|(i, t)| (i.to_string(), t));
            let returns = song.returns.iter().enumerate().filter(|(_, t)| !t.muted).map(|(i, t)| (format!("r{i}"), t));
            for (number, (label, strip)) in strips.chain(returns).enumerate() {
                let name: String = strip.name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
                outputs.push(json!({"id":label, "endpoint":{"kind":"node", "id":format!("tap_{label}")}, "path":format!("{stem}-{:02}-{name}.wav", number + 1)}));
            }
        }
        let job = self.call("render.start", json!({
            "source":"", "events":events, "frames":frames, "sample_rate":sample_rate as u32, "outputs":outputs,
        }))?["id"].as_u64().ok_or("The engine did not start the render")?;
        loop {
            thread::sleep(Duration::from_millis(50));
            let state = self.call("jobs.status", json!({"id":job}))?;
            match state["state"].as_str() {
                Some("completed") => return Ok(()),
                Some("running") => {}
                _ => return Err(format!("Export failed: {}", state["error"].as_str().unwrap_or("the render stopped"))),
            }
        }
    }

    fn tick(&mut self) -> Result<(), String> {
        let sample = self.clock()?;
        let sr = self.status.sample_rate;
        if let Some(playback) = &mut self.playback {
            let position = playback.position(sample, sr);
            self.status.row = position.as_ref().map(|p| p.row);
            self.status.row_phase = playback.phase(sample, sr);
            self.status.row_at_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
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
pub fn audio_scopes(audio: tauri::State<'_, Audio>) -> Scopes {
    audio.scopes()
}

#[tauri::command]
pub fn audio_status(audio: tauri::State<'_, Audio>) -> Status {
    audio.status()
}

#[cfg(test)]
#[path = "audio_tests.rs"]
mod tests;
