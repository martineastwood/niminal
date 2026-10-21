//! The engine: a running mixer that timestamped events are sent to.
//!
//! A client sends notes, parameter changes, control changes, definitions and releases, each
//! stamped with the absolute sample it is for, and the engine plays them. It knows nothing of
//! clips, scenes, patterns or when a change should wait for a bar line: those are for the client to work
//! out. Changes of definitions compile when they are sent and land at the sample they
//! name; everything else is put in a queue or in the window being prepared.
//!
//! The session is driven by a sample clock rather than a real one, so it is deterministic: the same inputs
//! at the same samples give the same audio, however the audio is divided into blocks.

use std::collections::{HashMap, VecDeque};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use niminal_engine::{
    BLOCK, DEFAULT_CEILING, DEFAULT_MAX_VOICES, MAX_CHANNELS, Master, Mixer, Transfer,
};
use niminal_lang::{
    CompileOptions, Expanded, Layout, Program, Samples, Statement, analyze,
    compile_with,
};
use niminal_score::{Event, Time, Value};

use crate::engine_server::ClientId;
use crate::events::{At, Cancelled, Op, Sent};
use crate::log::{LogEntry, LogInput};
use crate::sounding::Sounding;
use niminal_lang::{Entry, Located, Problem, Project};

/// How many MIDI messages are kept waiting to be taken, at most.
const MAX_MIDI_QUEUE: usize = 65_536;

/// The key of a note that is not held open by one.
pub(crate) const NO_KEY: u32 = u32::MAX;

/// A note waiting for its moment.
struct Upcoming {
    activity: Option<Arc<niminal_engine::Activity>>,
    start: u64,
    dur: u64,
    event: Event,
    /// A note held until it is let go of is named by this key, whatever `dur` says (unless `dur` is given,
    /// in which case that is the longest it is held).
    key: Option<u32>,
    /// The client that sent it, if one did, and the tag it gave.
    client: Option<ClientId>,
    tag: Option<String>,
}

/// A note a client holds open or may change, by the id the client chose.
struct VoiceEntry {
    destination: Option<String>,
    client: ClientId,
    id: u64,
    key: u32,
    /// The instrument it plays, by name so that it survives the program being replaced.
    instrument: String,
    schema: Arc<niminal_lang::Instrument>,
    tag: Option<String>,
}

/// A change to a parameter of the note a key holds. The parameter is found by name in the instrument
/// when the change is planned.
#[derive(Clone)]
pub(crate) struct VoiceSet {
    pub key: u32,
    pub name: String,
    pub value: f32,
    pub frames: u64,
    pub at: u64,
    done: bool,
    client: ClientId,
    tag: Option<String>,
}

#[derive(Clone)]
pub(crate) struct VoiceMap {
    key: u32,
    param: String,
    source: Option<String>,
    channel: usize,
    scale: f32,
    offset: f32,
    range: Option<(f32, f32)>,
    at: u64,
    client: ClientId,
    tag: Option<String>,
    done: bool,
}

/// A node parameter, which is a control, moving at a sample: a lane or the `node` event. Planned
/// into the windows like notes, since a lane can move one a great many times.
#[derive(Clone)]
pub(crate) struct NodeSet {
    pub key: String,
    pub value: f32,
    pub frames: u64,
    pub at: u64,
    client: Option<ClientId>,
    tag: Option<String>,
    done: bool,
}

#[derive(Clone)]
pub(crate) struct BufferSet {
    name: String,
    offset: usize,
    values: Arc<[f32]>,
    at: u64,
    client: ClientId,
    tag: Option<String>,
    done: bool,
}

/// A panic asked for at a sample.
#[derive(Clone)]
struct TimedPanic {
    at: u64,
    client: ClientId,
    tag: Option<String>,
    done: bool,
}

#[derive(Clone)]
struct ChainMap {
    client: u64,
    node: String,
    param: String,
    mapping: VoiceMap,
}
impl ChainMap {
    fn node(&self) -> String {
        format!("__routing_processor_{}_{}", self.client, self.node)
    }
}
#[derive(Clone)]
struct BufferBind {
    param: String,
    target: Option<String>,
    key: u32,
    resource: u64,
    bank: niminal_engine::BufferBank,
    count: usize,
    at: u64,
    client: u64,
    tag: Option<String>,
    done: bool,
}
impl BufferBind {
    fn banks(&self) -> Vec<niminal_engine::BufferBank> {
        vec![self.bank.clone(); self.count]
    }
}

/// A waiting release of the note a key holds.
struct KeyOff {
    key: u32,
    at: u64,
    client: Option<ClientId>,
    tag: Option<String>,
}

/// Who sent a control change that has not happened yet.
struct ControlOwner {
    serial: u64,
    client: ClientId,
    tag: Option<String>,
}

pub(crate) mod definitions;
mod input;
mod parameters;
pub mod realtime;
mod transactions;

use definitions::{Meta, Pending, What, options};

/// Where the engine is in time, for a client that wants to schedule against it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClockInfo {
    /// The sample the output is at, as its audio callback last said; the render clock if it hasn't.
    pub sample: u64,
    pub sample_rate: f32,
    /// The sample up to which audio has been prepared.
    pub prepared_until: u64,
    /// How far ahead of the clock the engine prepares, in samples.
    pub lookahead: u64,
    /// Seconds from the audio callback to the speakers, as the audio API reports it.
    pub output_latency: f64,
    /// The Unix time in microseconds at which the audio callback was handed the buffer that starts at
    /// `sample`, or `None` before there is an output.
    pub wall_micros: Option<u64>,
    /// How many notes are sounding.
    pub voices: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EngineState {
    pub sample: u64,
    pub seconds: f64,
    pub voices: usize,
    pub pending: usize,
}

pub struct Session {
    buffer_budgets: HashMap<u64, usize>,
    buffer_allocations: std::cell::RefCell<Vec<(u64, std::sync::Weak<niminal_engine::Buffer>)>>,
    hidden_buffers: std::collections::HashSet<String>,
    pub(crate) activities: Vec<super::activity::Note>,
    pub(crate) voice_sequence: HashMap<u64, u64>,
    next_note: u64,
    controls: Vec<crate::controls::State>,
    control_changes: Vec<crate::controls::Change>,
    next_control: u64,
    key_offs: Vec<KeyOff>,
    /// Audio for nodes that read `input(..)`, from a file being rendered: a buffer for each channel, from sample 0.
    audio_input: Vec<Vec<f32>>,
    /// Notes sent to `midi_out` instruments and not yet taken.
    midi_out: Vec<crate::MidiEvent>,
    sample_rate: f32,
    layout: Layout,
    panic_epoch: Arc<AtomicU64>,
    /// Where the project's sample files are found, and the ones already read.
    samples: Samples,
    client_samples: HashMap<u64, Samples>,
    sample_dirs: Vec<std::path::PathBuf>,
    clock: u64,

    transactions: VecDeque<transactions::Transaction>,
    current_transaction: Option<Arc<AtomicU64>>,
    project: Project,
    program: Arc<Program>,
    definitions: Arc<Program>,
    routing: crate::routing::Routing,
    mixer: Mixer,
    master_meter: Arc<niminal_engine::Meter>,
    limiter: Option<Master>,

    upcoming: VecDeque<Upcoming>,
    /// Sounding notes: when each is released, and the key holding it open (or `NO_KEY`).
    sounding: Sounding,

    /// Changes of definitions, waiting to land.
    pending: Vec<Pending>,
    next_id: u64,
    notices: Vec<String>,
    recent_notices: Vec<(String, u64)>,
    log: Vec<LogEntry>,
    log_corrections: u64,
    silenced: usize,
    peaks: Vec<f32>,
    version: u64,
    live: Option<realtime::Planner>,
    detached: bool,
    /// Notes clients hold by id, and their parameter changes still to come.
    voices: Vec<VoiceEntry>,
    /// Notes whose release has been asked for but has not come yet, in case that is taken back.
    closed: Vec<VoiceEntry>,
    voice_sets: Vec<VoiceSet>,
    voice_maps: Vec<VoiceMap>,
    buffer_binds: Vec<BufferBind>,
    chain_maps: Vec<ChainMap>,
    /// Sorted by sample; recent applied entries remain available for replanning.
    node_sets: Vec<NodeSet>,
    buffer_sets: Vec<BufferSet>,
    timed_panics: Vec<TimedPanic>,
    control_owners: Vec<ControlOwner>,
    /// The key the next held note gets.
    next_key: u32,
    /// Where the audio output is, as its callback last said.
    probe: Arc<crate::ClockProbe>,
    /// Things added since the planner last looked, that it may have to put into windows already prepared.
    fresh: Vec<realtime::Fresh>,
    /// How many mixers have been built for the renderer.
    mixer_builds: Arc<std::sync::atomic::AtomicUsize>,
    /// Lets mixers share a block's voices among threads.
    pub(crate) fork: Option<Arc<dyn niminal_engine::Fork>>,
}

impl Session {
    /// An engine that outputs `layout` at `sample_rate`, with the limiter on.
    pub fn new(sample_rate: f32, layout: Layout) -> Session {
        let program = compile_with("", &options(layout, &Samples::default(), sample_rate))
            .expect("an empty project compiles");
        let mixer = program.mixer(sample_rate);
        Session {
            buffer_budgets: HashMap::new(),
            buffer_allocations: Default::default(),
            hidden_buffers: Default::default(),
            activities: Vec::new(),
            next_note: 1,
            voice_sequence: HashMap::new(),
            sample_rate,
            layout,
            panic_epoch: Arc::new(AtomicU64::new(0)),
            samples: Samples::default(),
            client_samples: HashMap::new(),
            controls: Vec::new(),
            control_changes: Vec::new(),
            next_control: 0,
            key_offs: Vec::new(),
            audio_input: Vec::new(),
            midi_out: Vec::new(),
            sample_dirs: Vec::new(),
            clock: 0,
            transactions: VecDeque::new(),
            current_transaction: None,
            project: Project::default(),
            definitions: Arc::new(program.clone()),
            routing: Default::default(),
            program: Arc::new(program),
            mixer,
            master_meter: Arc::default(),
            limiter: Some(Master::with_channels(
                sample_rate,
                DEFAULT_CEILING,
                layout.channels(),
            )),
            upcoming: VecDeque::new(),
            sounding: Sounding::default(),
            pending: Vec::new(),
            next_id: 1,
            notices: Vec::new(),
            recent_notices: Vec::new(),
            log: Vec::new(),
            log_corrections: 0,
            silenced: 0,
            peaks: vec![0.0; layout.channels()],
            version: 0,
            live: None,
            detached: false,
            voices: Vec::new(),
            closed: Vec::new(),
            voice_sets: Vec::new(),
            voice_maps: Vec::new(),
            buffer_binds: Vec::new(),
            chain_maps: Vec::new(),
            node_sets: Vec::new(),
            buffer_sets: Vec::new(),
            timed_panics: Vec::new(),
            control_owners: Vec::new(),
            next_key: 0,
            probe: Arc::default(),
            fresh: Vec::new(),
            mixer_builds: Arc::default(),
            fork: None,
        }
    }

    /// The sample files this session has read.
    pub fn buffer_budget(&self, client: u64) -> (usize, usize) {
        let mut allocations = self.buffer_allocations.borrow_mut();
        allocations.retain(|(_, b)| b.strong_count() > 0);
        let bytes = allocations
            .iter()
            .filter(|(owner, _)| *owner == client)
            .filter_map(|(_, b)| b.upgrade())
            .map(|b| b.len() * b.channels() * 4)
            .sum();
        (
            self.buffer_budgets
                .get(&client)
                .copied()
                .unwrap_or(256 * 1024 * 1024),
            bytes,
        )
    }
    pub fn set_buffer_budget(&mut self, client: u64, bytes: usize) -> Result<(), String> {
        if bytes < self.buffer_budget(client).1 {
            return Err("budget is below retained buffer storage".into());
        }
        self.buffer_budgets.insert(client, bytes);
        Ok(())
    }
    pub fn buffer_is_hidden(&self, name: &str) -> bool {
        self.hidden_buffers.contains(name)
    }
    pub fn hide_buffer(&mut self, name: String) {
        self.log.push(LogEntry {
            sample: self.clock,
            input: LogInput::FreeBuffer { name: name.clone() },
        });
        self.hidden_buffers.insert(name);
    }
    pub fn publish_buffer(&mut self, name: &str) {
        self.hidden_buffers.remove(name);
    }
    pub fn sample_files_for(&self, client: u64) -> &Samples {
        self.client_samples.get(&client).unwrap_or(&self.samples)
    }
    fn set_client_sample_dir(&mut self, client: u64, dir: &std::path::Path) {
        if client == 0 {
            self.set_sample_dir(dir);
            return;
        }
        let current = self
            .client_samples
            .entry(client)
            .or_insert_with(|| Samples::new(dir));
        let mut bases = current.bases().to_vec();
        bases.retain(|d| d != dir);
        bases.insert(0, dir.to_path_buf());
        *current = current.with_bases(bases);
    }
    pub fn sample_files(&self) -> &Samples {
        &self.samples
    }

    /// Find the paths in `sample` and `kit` declarations from `dir`. Folders given earlier are still
    /// searched, after this one.
    pub fn set_sample_dir(&mut self, dir: &std::path::Path) {
        if self.sample_dirs.first().is_none_or(|d| d != dir) {
            self.sample_dirs.retain(|d| d != dir);
            self.sample_dirs.insert(0, dir.to_path_buf());
            self.samples = self.samples.with_bases(self.sample_dirs.clone());
        }
    }

    /// Turn the master limiter off. Only for offline use.
    /// Have mixers share a block's voices among the threads `fork` runs jobs on. The sound does not change.
    pub fn set_fork(&mut self, fork: Option<Arc<dyn niminal_engine::Fork>>) {
        self.fork = fork;
        self.mixer.set_fork(self.fork.clone());
    }

    pub(crate) fn new_mixer(&self, program: &Program) -> Mixer {
        let mut mixer = program.mixer(self.sample_rate);
        mixer.set_fork(self.fork.clone());
        mixer
    }

    pub fn without_limiter(mut self) -> Session {
        self.limiter = None;
        self
    }

    /// Samples of delay the limiter adds to the output.
    pub fn latency(&self) -> usize {
        self.limiter
            .as_ref()
            .map(Master::latency)
            .unwrap_or_else(|| self.live.as_ref().map_or(0, |p| p.state.latency))
    }

    /// For the audio callback to say where the output is.
    pub fn clock_probe(&self) -> Arc<crate::ClockProbe> {
        self.probe.clone()
    }

    pub fn channels(&self) -> usize {
        self.layout.channels()
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn clock(&self) -> u64 {
        self.clock
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    /// The program an event will execute on. Late events execute now, including
    /// definitions waiting at the current sample but not yet consumed by DSP.
    pub(super) fn program_at(&self, at: u64) -> &Arc<Program> {
        let at = at.max(self.clock);
        self.pending
            .iter()
            .rev()
            .find(|p| p.lands_at <= at)
            .map(|p| &p.program)
            .unwrap_or(&self.program)
    }

    fn routing_at(&self, at: u64) -> &crate::routing::Routing {
        let at = at.max(self.clock);
        self.pending
            .iter()
            .rev()
            .find(|p| p.lands_at <= at)
            .map_or(&self.routing, |p| &p.routing)
    }

    pub fn master_meter(&self) -> Arc<niminal_engine::Meter> {
        self.master_meter.clone()
    }

    pub fn endpoint_meter(
        &self,
        client: u64,
        endpoint: &crate::Endpoint,
    ) -> Result<Arc<niminal_engine::Meter>, String> {
        if matches!(endpoint, crate::Endpoint::Master) {
            Ok(self.master_meter())
        } else {
            self.routing.meter(client, endpoint)
        }
    }

    pub fn routing_meters(&self, take: bool) -> serde_json::Value {
        self.routing.meters(take)
    }

    pub fn routing_status(&self) -> serde_json::Value {
        self.routing.status(&self.control_values())
    }

    fn seconds(&self, sample: u64) -> f64 {
        sample as f64 / f64::from(self.sample_rate)
    }

    fn samples(&self, seconds: f64) -> u64 {
        (seconds * f64::from(self.sample_rate)).round().max(0.0) as u64
    }

    // ---- inputs -------------------------------------------------------------

    /// Changes on an immediate panic, allowing an output queue to discard audio
    /// that was rendered before it. Scheduled panics are already in the audio.
    pub fn panic_epoch(&self) -> Arc<AtomicU64> {
        self.panic_epoch.clone()
    }

    /// Stop everything at once, including effect tails, and forget what was waiting.
    pub fn panic(&mut self) {
        self.sync_realtime();
        for key in self.routing.player_keys(None) {
            if let Some(control) = self.controls.iter_mut().find(|c| c.key == key) {
                control.set(self.clock, 0., 0);
            }
        }
        self.panic_epoch.fetch_add(1, Ordering::Release);
        self.invalidate_live();
        self.log.push(LogEntry {
            sample: self.clock,
            input: LogInput::Panic,
        });
        self.upcoming.clear();
        self.node_sets.clear();
        self.buffer_sets.clear();
        self.voice_sets.clear();
        self.voice_maps.clear();
        self.buffer_binds.clear();
        self.chain_maps.clear();
        self.key_offs.clear();
        self.voices.clear();
        self.closed.clear();
        self.silence();
    }

    pub fn panic_client(&mut self, client: ClientId) {
        let exclusive = self.client_status().iter().all(|(id, _, _)| *id == client)
            && self
                .program
                .instruments
                .iter()
                .all(|i| definitions::local(client, &i.name).is_some())
            && self
                .program
                .nodes
                .iter()
                .skip(1)
                .all(|t| definitions::owns_node(client, &t.name))
            && self.program.controls.iter().all(|c| {
                definitions::local(client, &c.name).is_some()
                    || definitions::owns_node(client, &c.name)
            });
        // With no other owner's audio, the mixed limiter history can also be discarded.
        if exclusive {
            self.panic();
            return;
        }

        self.cancel_events(client, None, At::Now, true);
        let _ = self.send_events(
            client,
            vec![Op::Panic {
                at: At::Now,
                tag: None,
            }],
        );
    }
    fn silence_client(&mut self, client: ClientId) {
        if self.detached {
            return;
        }
        let (ended, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.sounding.midi)
            .into_iter()
            .partition(|m| m.3 == client);
        self.sounding.midi = kept;
        for (_, _, note, _) in ended {
            self.send_midi(false, &note, self.clock);
        }
        for key in self.routing.player_keys(Some(client)) {
            if let Some(control) = self.controls.iter_mut().find(|c| c.key == key) {
                control.set(self.clock, 0., 0);
            }
        }
        let mut mixer = self.new_mixer(&self.program);
        let mut transfers = Vec::new();
        plan_transfers(&self.program, &self.program, &mut transfers);
        self.mixer.silence_owner(client);
        for (node, transfer) in self.program.nodes.iter().zip(&mut transfers) {
            if !node.name.is_empty() && definitions::owns_node(client, &node.name) {
                transfer.chain_from = None;
                transfer.voices_from = None;
            }
        }
        mixer.adopt(&mut self.mixer, &transfers);
        crate::controls::sync_mixer(&self.controls, &mut mixer, self.clock);
        self.mixer = mixer;
    }

    /// Stop every voice and clear the memory of every effect.
    fn silence(&mut self) {
        if !self.detached {
            self.mixer.finish_activities(self.clock, 2);
            self.mixer.panic();
            crate::controls::sync_mixer(&self.controls, &mut self.mixer, self.clock);
        }
        if let Some(limiter) = &mut self.limiter {
            limiter.clear();
        }
        self.mixer.finish_activities(self.clock, 2);
        self.sounding.held.clear();
        for (_, _, note, _) in std::mem::take(&mut self.sounding.midi) {
            if self.midi_out.len() < MAX_MIDI_QUEUE {
                self.midi_out.push(crate::MidiEvent {
                    at: self.clock,
                    on: false,
                    port: note.port,
                    channel: note.channel,
                    note: note.note,
                    velocity: note.velocity,
                });
            }
        }
    }

    pub fn control_values(&self) -> Vec<(String, f32, f32, Option<&'static str>)> {
        self.program
            .controls
            .iter()
            .zip(&self.controls)
            .map(|(def, state)| {
                (
                    def.name.clone(),
                    state.value(self.clock),
                    state.target,
                    def.unit.suffix(),
                )
            })
            .collect()
    }

    // ---- state ----------------------------------------------------------------

    pub fn clock_info(&mut self) -> ClockInfo {
        self.sync_realtime();
        let probe = self.probe.read();
        let sample = probe.map_or(self.clock, |(sample, ..)| sample);
        ClockInfo {
            sample,
            sample_rate: self.sample_rate,
            prepared_until: self.prepared_until(),
            lookahead: self.live.as_ref().map_or(0, |p| p.lookahead()),
            output_latency: probe.map_or(0.0, |(_, _, micros)| micros as f64 / 1e6),
            wall_micros: probe.map(|(_, wall, _)| wall),
            voices: self.state().voices,
        }
    }

    /// The instruments and nodes that are defined, with their parameters, for a client that wants to offer them.
    pub fn definitions(&self) -> (Vec<niminal_lang::Instrument>, Vec<String>, Vec<String>) {
        (
            self.program.instruments.clone(),
            self.program
                .nodes
                .iter()
                .skip(1)
                .map(|t| t.name.clone())
                .collect(),
            self.program.buses.clone(),
        )
    }

    /// Each client's held notes (by the ids it chose) and how many of its events are still waiting.
    pub fn client_status(&self) -> Vec<(ClientId, Vec<u64>, usize)> {
        let mut clients: Vec<ClientId> = self.voices.iter().map(|v| v.client).collect();
        clients.extend(self.upcoming.iter().filter_map(|u| u.client));
        clients.extend(self.control_owners.iter().map(|o| o.client));
        clients.extend(self.pending.iter().map(|p| p.meta.client));
        clients.extend(
            self.buffer_sets
                .iter()
                .filter(|s| !s.done)
                .map(|s| s.client),
        );
        clients.extend(self.voice_maps.iter().filter(|s| !s.done).map(|s| s.client));
        clients.sort_unstable();
        clients.dedup();
        clients
            .into_iter()
            .map(|c| {
                let ids = self
                    .voices
                    .iter()
                    .filter(|v| v.client == c)
                    .map(|v| v.id)
                    .collect();
                let waiting = self
                    .upcoming
                    .iter()
                    .filter(|u| u.client == Some(c) && u.start >= self.clock)
                    .count()
                    + self.control_owners.iter().filter(|o| o.client == c).count()
                    + self
                        .voice_sets
                        .iter()
                        .filter(|s| s.client == c && !s.done)
                        .count()
                    + self
                        .node_sets
                        .iter()
                        .filter(|s| s.client == Some(c) && !s.done)
                        .count()
                    + self.pending.iter().filter(|p| p.meta.client == c).count()
                    + self
                        .buffer_sets
                        .iter()
                        .filter(|s| s.client == c && !s.done)
                        .count()
                    + self
                        .voice_maps
                        .iter()
                        .filter(|s| s.client == c && !s.done)
                        .count();
                (c, ids, waiting)
            })
            .collect()
    }

    /// How many changes of definitions are waiting to land.
    pub fn waiting_changes(&self) -> usize {
        self.pending.len()
    }

    pub fn state(&self) -> EngineState {
        EngineState {
            sample: self.clock,
            seconds: self.seconds(self.clock),
            voices: self.live.as_ref().map_or_else(
                || self.mixer.active_voices(),
                |p| p.state.voices.load(Ordering::Relaxed),
            ),
            pending: self.pending.len(),
        }
    }

    /// Queue a notice, unless the same one is already waiting: a problem that
    /// recurs every fraction of a second must not bury everything else.
    pub(crate) fn notify(&mut self, notice: String) {
        let quiet_until = u64::from(self.sample_rate as u32) * NOTICE_REPEAT_SECONDS;
        self.recent_notices
            .retain(|(_, at)| self.clock.saturating_sub(*at) < quiet_until);
        if self.notices.len() < MAX_NOTICES
            && !self.recent_notices.iter().any(|(n, _)| *n == notice)
        {
            self.recent_notices.push((notice.clone(), self.clock));
            self.notices.push(notice);
        }
    }

    /// Things worth telling the user that aren't errors in what they sent: a note that couldn't play, a
    /// change that failed to land.
    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    /// The audio that nodes reading `input(..)` hear, a buffer for each channel starting at sample 0 of the
    /// session. For rendering a file; a live session listens to a device instead.
    pub fn set_audio_input(&mut self, channels: Vec<Vec<f32>>) {
        self.audio_input = channels;
    }

    /// Start keeping what each node and bus puts out, for an offline render's stems. Only for renders:
    /// it grows without bound.
    pub fn capture_stems(&mut self) {
        self.mixer.capture_stems();
    }

    /// The stems kept so far, no longer kept after this.
    pub fn take_stems(&mut self) -> Option<niminal_engine::Stems> {
        self.mixer.take_stems()
    }

    /// Voices silenced for producing NaN or infinity since this was last called.
    pub fn take_silenced(&mut self) -> usize {
        std::mem::take(&mut self.silenced)
    }

    /// The loudest sample on each output channel since this was last called.
    pub fn take_peaks(&mut self) -> Vec<f32> {
        self.sync_realtime();
        std::mem::replace(&mut self.peaks, vec![0.0; self.layout.channels()])
    }

    /// Changes when entries are appended or actual control timing is acknowledged.
    pub fn log_version(&self) -> (usize, u64) {
        (self.log.len(), self.log_corrections)
    }

    /// Everything that has been sent to the engine, with when.
    pub fn log(&self) -> &[LogEntry] {
        &self.log
    }

    // ---- landing ------------------------------------------------------------------------

    fn land_due(&mut self, before: u64) {
        while self.pending.first().is_some_and(|p| {
            p.lands_at <= self.clock
                && (p.lands_at < before
                    || p.applied.load(Ordering::Acquire) != transactions::WAITING)
        }) {
            let pending = self.pending.remove(0);
            self.land(pending);
        }
    }

    fn land(&mut self, p: Pending) {
        self.version += 1;
        let Pending {
            id,
            statements,
            project,
            program,
            mixer,
            definitions,
            routing,
            applied,
            ..
        } = p;
        if !self.detached {
            applied.store(self.clock, Ordering::Release);
        }
        self.current_transaction = Some(applied);
        self.reconcile_transactions();
        for statement in &statements {
            if let Some(name) = statement.key.strip_prefix("buffer:") {
                self.hidden_buffers.remove(name);
            }
        }
        self.definitions = definitions;
        self.routing = routing;
        let released = self
            .voices
            .iter()
            .filter(|v| {
                v.destination.as_ref().is_some_and(|name| {
                    !program
                        .nodes
                        .iter()
                        .any(|t| t.name == *name && t.accepts_notes)
                })
            })
            .map(|v| v.key)
            .collect::<Vec<_>>();
        self.voices.retain(|v| !released.contains(&v.key));
        for key in released {
            self.release_at(key, self.clock, None);
        }

        self.swap_program(project, program, mixer, &statements, id);
    }

    /// Replace the running program, keeping what is sounding.
    fn swap_program(
        &mut self,
        project: Project,
        program: Arc<Program>,
        mut mixer: Mixer,
        statements: &[Statement],
        revision: u64,
    ) {
        let mut controls = crate::controls::build(&program);
        crate::controls::mark(&mut controls, &program, statements, revision);
        crate::controls::adopt(
            &mut controls,
            &self.controls,
            &program,
            self.clock,
            self.sample_rate,
        );
        self.controls = controls;
        if self.detached {
            self.program = program;
            self.project = project;
            return;
        }
        let mapping: Vec<Option<usize>> = self
            .program
            .buses
            .iter()
            .enumerate()
            .map(|(i, name)| {
                program
                    .buses
                    .iter()
                    .position(|n| n == name)
                    .filter(|&j| self.program.bus_layouts[i] == program.bus_layouts[j])
            })
            .collect();
        self.mixer.remap_buses(&mapping);
        let mut transfers = Vec::new();
        plan_transfers(&self.program, &program, &mut transfers);
        mixer.adopt(&mut self.mixer, &transfers);
        crate::controls::sync_mixer(&self.controls, &mut mixer, self.clock);
        self.mixer.finish_activities(self.clock, 4);
        self.mixer = mixer;
        self.program = program;
        self.restore_chain_bindings();
        self.project = project;
    }

    // ---- running ---------------------------------------------------------------------------

    /// A number for a note that is held until it is let go of, never `NO_KEY`.
    fn new_key(&mut self) -> u32 {
        let key = self.next_key;
        self.next_key = self.next_key.wrapping_add(1) % NO_KEY;
        key
    }

    fn insert_upcoming(&mut self, note: Upcoming) {
        let at = self.upcoming.partition_point(|u| u.start <= note.start);
        self.upcoming.insert(at, note);
    }

    /// Mix `frames` samples, calling `sink` with each piece (a block per
    /// channel and how many of its samples are valid).
    pub fn render(&mut self, frames: usize, sink: &mut dyn FnMut(&[[f32; BLOCK]], usize)) {
        assert!(
            !self.detached,
            "a detached session is driven by its renderer"
        );
        // the same mode as the live renderer, so that the two render alike
        let _flush = niminal_engine::denormals::FlushToZero::new();
        let end = self.clock + frames as u64;
        let channels = self.layout.channels();
        let mut block = [[0.0f32; BLOCK]; MAX_CHANNELS];

        while self.clock < end {
            self.mixer
                .set_clock(niminal_engine::Clock { sample: self.clock });
            self.land_due(self.clock.saturating_add(1));
            self.apply_controls();
            self.release_due();
            self.apply_voice_sets();
            self.apply_node_sets(self.clock.saturating_add(1));
            for set in &mut self.buffer_sets {
                if !set.done && set.at <= self.clock {
                    if let Some((_, buffer)) = self
                        .program
                        .buffers
                        .iter()
                        .find(|(name, _)| *name == set.name)
                        && set.offset <= buffer.len()
                        && set.values.len() <= (buffer.len() - set.offset) * buffer.channels()
                    {
                        for (i, value) in set.values.iter().enumerate() {
                            buffer.write_sample(set.offset * buffer.channels() + i, *value);
                        }
                    }
                    set.done = true;
                }
            }
            let stale = self.clock.saturating_sub(self.sample_rate as u64 * 2);
            self.buffer_sets.retain(|s| !s.done || s.at > stale);
            self.apply_timed_panics();

            // How far we can go before something needs doing.
            let mut n = (end - self.clock).min(BLOCK as u64);
            if let Some(p) = self.pending.first() {
                n = n.min(p.lands_at - self.clock);
            }
            if let Some(change) = self.control_changes.first() {
                n = n.min(change.at.saturating_sub(self.clock).max(1));
            }
            n = self.sounding.limit(self.clock, n);
            let upcoming = self
                .key_offs
                .iter()
                .map(|k| k.at)
                .chain(self.voice_sets.iter().filter(|s| !s.done).map(|s| s.at))
                .chain(self.node_sets.iter().filter(|s| !s.done).map(|s| s.at))
                .chain(
                    self.chain_maps
                        .iter()
                        .filter(|m| !m.mapping.done)
                        .map(|m| m.mapping.at),
                )
                .chain(self.buffer_binds.iter().filter(|b| !b.done).map(|b| b.at))
                .chain(self.timed_panics.iter().filter(|p| !p.done).map(|p| p.at))
                .chain(self.buffer_sets.iter().filter(|s| !s.done).map(|s| s.at))
                .chain(self.voice_maps.iter().filter(|s| !s.done).map(|s| s.at));
            for at in upcoming.filter(|&at| at > self.clock) {
                n = n.min(at - self.clock);
            }
            if let Some(next) = self.upcoming.iter().find(|u| u.start > self.clock) {
                n = n.min(next.start - self.clock);
            }
            self.start_due();
            self.apply_buffer_binds();
            self.apply_chain_maps();
            self.apply_voice_maps();

            let n = n.max(1) as usize;
            for (c, samples) in self
                .audio_input
                .iter()
                .enumerate()
                .take(niminal_engine::MAX_CHANNELS)
            {
                let from = (self.clock as usize).min(samples.len());
                self.mixer
                    .set_audio_input(c, &samples[from..(from + n).min(samples.len())]);
            }
            self.mixer
                .set_clock(niminal_engine::Clock { sample: self.clock });
            self.mixer.process(&mut block[..channels], n);
            self.silenced += self.mixer.take_silenced();
            if let Some(limiter) = &mut self.limiter {
                limiter.process_block(&mut block[..channels], n);
            }
            for (peak, b) in self.peaks.iter_mut().zip(&block) {
                *peak = b[..n].iter().fold(*peak, |m, s| m.max(s.abs()));
            }
            self.master_meter.observe(&block[..channels], n, self.clock);
            sink(&block[..channels], n);
            self.clock += n as u64;
        }
        self.land_due(self.clock.saturating_add(1));
    }

    /// Run a recorded session from the start and return its audio, `frames`
    /// samples of it. Because inputs are applied at the sample they were first
    /// applied at, this is the audio the original produced, however it was
    /// divided into blocks.
    pub fn replay(
        sample_rate: f32,
        layout: Layout,
        log: &[LogEntry],
        frames: u64,
        limiter: bool,
    ) -> Vec<Vec<f32>> {
        let mut session = Session::new(sample_rate, layout);
        if !limiter {
            session = session.without_limiter();
        }
        let mut out = vec![Vec::new(); layout.channels()];
        let mut extend = |session: &mut Session, upto: u64| {
            let gap = upto.saturating_sub(session.clock) as usize;
            for (o, p) in out.iter_mut().zip(session.process(gap)) {
                o.extend(p);
            }
        };
        for entry in log {
            extend(&mut session, entry.sample);
            // Rejected inputs were rejected the first time too.
            match &entry.input {
                LogInput::Send { client, ops } => {
                    let _ = session.send_events(*client, ops.clone());
                }
                LogInput::CancelEvents {
                    client,
                    tag,
                    from,
                    release,
                } => {
                    session.cancel_events(*client, tag.as_deref(), At::Sample(*from), *release);
                }
                LogInput::Panic => session.panic(),
                LogInput::FreeBuffer { name } => session.hide_buffer(name.clone()),
            }
        }
        extend(&mut session, frames);
        out
    }

    /// Mix `frames` samples and return them, one buffer per channel.
    pub fn process(&mut self, frames: usize) -> Vec<Vec<f32>> {
        let mut out = vec![Vec::with_capacity(frames); self.layout.channels()];
        self.render(frames, &mut |block, n| {
            for (o, b) in out.iter_mut().zip(block) {
                o.extend_from_slice(&b[..n]);
            }
        });
        out
    }

    fn start_due(&mut self) {
        while self.upcoming.front().is_some_and(|u| u.start <= self.clock) {
            let Some(note) = self.upcoming.pop_front() else {
                break;
            };
            match self.program.plan(&note.event) {
                Ok(plan) => {
                    let graph = self.program.instruments[plan.instrument].graph.clone();
                    for (key, value, smooth) in &plan.controls {
                        let frames = (smooth * f64::from(self.sample_rate)).round() as u64;
                        if let Some(c) = self.controls.iter_mut().find(|c| c.key == *key) {
                            c.set(self.clock, *value, frames);
                            self.mixer
                                .set_control(&c.key, c.start, c.target, c.frames, 0);
                        }
                    }
                    let id = self.mixer.note_on_with(
                        plan.node,
                        graph,
                        &plan.params,
                        plan.choke,
                        plan.rule,
                    );
                    if let Some(activity) = note.activity {
                        self.mixer.attach_voice_activity(id, activity, self.clock);
                    }
                    self.mixer.set_voice_owner(id, note.client.unwrap_or(0));
                    self.mixer.set_voice_pan(id, plan.pan);
                    self.mixer.set_voice_dur(
                        id,
                        if note.key.is_some() && note.dur == 0 {
                            0.0
                        } else {
                            note.dur as f32 / self.sample_rate
                        },
                    );
                    crate::controls::sync_note(&self.controls, &mut self.mixer, id, self.clock);
                    let off = if note.key.is_some() && note.dur == 0 {
                        u64::MAX
                    } else {
                        self.clock + note.dur
                    };
                    self.sounding
                        .held
                        .push((off, id, note.key.unwrap_or(NO_KEY)));
                    if let Some(midi) = plan.midi {
                        self.send_midi(true, &midi, self.clock);
                        self.sounding.midi.push((
                            off,
                            note.key.unwrap_or(NO_KEY),
                            midi,
                            note.client.unwrap_or(0),
                        ));
                    }
                    if self.mixer.take_stolen() > 0 {
                        self.notify(format!("too many notes at once: the oldest are ended early to keep to {} voices", DEFAULT_MAX_VOICES));
                    }
                }
                Err(e) => self.notify(format!(
                    "note for `{}` could not play: {e}",
                    note.event.target
                )),
            }
        }
    }

    fn send_midi(&mut self, on: bool, note: &niminal_lang::MidiNote, at: u64) {
        if self.midi_out.len() < MAX_MIDI_QUEUE {
            self.midi_out.push(crate::MidiEvent {
                at,
                on,
                port: note.port.clone(),
                channel: note.channel,
                note: note.note,
                velocity: note.velocity,
            });
        }
    }

    /// The notes that went to `midi_out` instruments since this was last called, in order.
    pub fn take_midi(&mut self) -> Vec<crate::MidiEvent> {
        std::mem::take(&mut self.midi_out)
    }

    /// Silence everything at the samples panics were asked for.
    fn apply_timed_panics(&mut self) {
        let now = self.clock;
        let clients: Vec<_> = self
            .timed_panics
            .iter_mut()
            .filter(|p| !p.done && p.at <= now)
            .map(|p| {
                p.done = true;
                p.client
            })
            .collect();
        let stale = now.saturating_sub(self.sample_rate as u64 * 2);
        self.timed_panics.retain(|p| !p.done || p.at + 1 > stale);
        for client in clients {
            self.silence_client(client);
        }
    }

    /// Move the node parameters that `node` events say should move by now.
    // `before` is exclusive: metadata must not consume a control at the renderer's next, unrendered sample.
    fn apply_node_sets(&mut self, before: u64) {
        let now = self.clock;
        for set in &mut self.node_sets {
            if set.at >= before {
                break;
            }
            if set.done {
                continue;
            }
            if let Some(state) = self.controls.iter_mut().find(|c| c.key == set.key) {
                state.set(set.at, set.value, set.frames);
                self.mixer.set_control(
                    &state.key,
                    state.start,
                    state.target,
                    state.frames,
                    now.saturating_sub(set.at),
                );
            }
            set.done = true;
        }
        let stale = now.saturating_sub(self.samples(2.0));
        self.node_sets.retain(|s| !s.done || s.at > stale);
    }

    /// Move the parameters of held notes that their changes say should move by now.
    fn apply_voice_sets(&mut self) {
        let now = self.clock;
        for i in 0..self.voice_sets.len() {
            let set = &self.voice_sets[i];
            if set.done || set.at > now {
                continue;
            }
            for held in self.sounding.held.iter().filter(|h| h.2 == set.key) {
                self.mixer
                    .set_voice_param_glide(held.1, &set.name, set.value, set.frames);
            }
            self.voice_sets[i].done = true;
        }
        let stale = now.saturating_sub(self.samples(2.0));
        self.voice_sets.retain(|s| !s.done || s.at > stale);
    }

    fn release_due(&mut self) {
        let now = self.clock;
        for off in &self.key_offs {
            if off.at <= now {
                self.sounding.release_key(off.key, off.at);
            }
        }
        let (ended, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.sounding.midi)
            .into_iter()
            .partition(|h| h.0 <= now);
        self.sounding.midi = kept;
        for (_, _, note, _) in &ended {
            self.send_midi(false, note, now);
        }
        let mixer = &mut self.mixer;
        self.sounding.held.retain(|&(off, id, _)| {
            if off <= now {
                mixer.release(id);
            }
            off > now
        });
    }
}

/// Which running voices and effect chains the new program's nodes take over from the old one's.
pub(super) fn plan_transfers(old: &Program, new: &Program, out: &mut Vec<Transfer>) {
    out.clear();
    for (i, node) in new.nodes.iter().enumerate() {
        let from = if i == 0 {
            Some(0)
        } else {
            old.node_index(&node.name)
        };
        out.push(Transfer {
            release: !node.accepts_notes && from.is_some_and(|i| old.nodes[i].accepts_notes),
            voices_from: from,
            chain_from: from.filter(|_| i > 0),
        });
    }
}

/// The most notices that wait to be collected, and how long before the same one is said again.
const MAX_NOTICES: usize = 100;
const NOTICE_REPEAT_SECONDS: u64 = 10;
