//! Prepared event windows into an exclusively owned DSP renderer. Both queues
//! are bounded: one delivers constructed voices and mixers, the other returns
//! their storage for destruction on the planning thread.

use super::*;
use crate::controls::{self, Applied, Change, State as ControlState};
use crate::sounding::Sounding;
use niminal_engine::Voice;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::atomic::{AtomicU32, AtomicUsize};

#[derive(Clone, Copy, Debug)]
pub struct RealtimeConfig {
    pub lookahead_frames: u64,
    pub window_frames: usize,
    pub queued_windows: usize,
    pub max_voices: usize,
}

/// How the first sample of a prepared window relates to the live mixer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlanMode {
    Advance,
    /// Plan again from now with the mixer swapped for a new one, because definitions changed.
    Refresh,
    /// Plan again from now, keeping the mixer, because something waiting was taken back.
    Replan,
    Reset,
}

impl Default for RealtimeConfig {
    fn default() -> Self {
        Self {
            lookahead_frames: 8192,
            window_frames: 2048,
            queued_windows: 32,
            max_voices: 512,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderMetrics {
    pub late_events: usize,
    pub starved_frames: usize,
    pub capacity_drops: usize,
    pub control_drops: usize,
    pub retire_pressure: usize,
    pub deadline_misses: usize,
    pub planner_panics: usize,
    pub max_render_micros: u64,
    /// Mixers built for the renderer: one for each change of definitions, and for each replan.
    pub mixer_builds: usize,
}

pub(super) struct State {
    clock: AtomicU64,
    revision: AtomicU64,
    /// Changes along with `revision` when the mixer too must be swapped.
    swap_epoch: AtomicU64,
    reset_epoch: AtomicU64,
    pub(super) voices: AtomicUsize,
    peaks: [AtomicU32; MAX_CHANNELS],
    silenced: AtomicUsize,
    late_events: AtomicUsize,
    starved_frames: AtomicUsize,
    capacity_drops: AtomicUsize,
    control_drops: AtomicUsize,
    retire_pressure: AtomicUsize,
    deadline_misses: AtomicUsize,
    planner_panics: AtomicUsize,
    max_render_micros: AtomicU64,
    pub(super) latency: usize,
    mixer_builds: Arc<AtomicUsize>,
    /// The thread that plans ahead, woken when something changes that it must act on at once.
    planner_thread: std::sync::Mutex<Option<std::thread::Thread>>,
}

impl State {
    fn metrics(&self) -> RenderMetrics {
        RenderMetrics {
            late_events: self.late_events.load(Ordering::Relaxed),
            starved_frames: self.starved_frames.load(Ordering::Relaxed),
            capacity_drops: self.capacity_drops.load(Ordering::Relaxed),
            control_drops: self.control_drops.load(Ordering::Relaxed),
            retire_pressure: self.retire_pressure.load(Ordering::Relaxed),
            deadline_misses: self.deadline_misses.load(Ordering::Relaxed),
            planner_panics: self.planner_panics.load(Ordering::Relaxed),
            max_render_micros: self.max_render_micros.load(Ordering::Relaxed),
            mixer_builds: self.mixer_builds.load(Ordering::Relaxed),
        }
    }
}

struct Window {
    end: u64,
    revision: u64,
    commands: Vec<Command>,
    notices: Vec<String>,
    stopped: Vec<String>,
    dropped: usize,
}

impl Window {
    /// Say something once per window, however many notes it applies to.
    fn notice(&mut self, text: &str) {
        if !self.notices.iter().any(|n| n == text) {
            self.notices.push(text.into());
        }
    }
}

/// The most changes envelope lanes may make to parameters in one prepared window.
const MAX_WINDOW_CONTROLS: usize = 2048;

struct Command {
    at: u64,
    kind: Kind,
}

impl Command {
    /// Commands at one sample go in this order: a reset or swap of the graph before the notes it applies to.
    fn order(&self) -> (u64, u8) {
        (
            self.at,
            match self.kind {
                Kind::Reset { .. } => 0,
                Kind::Swap { .. } | Kind::Control { .. } | Kind::Buffer { .. } => 1,
                Kind::Note { .. } => 2,
                Kind::Release { .. } | Kind::SetParam { .. } | Kind::Map { .. } => 3,
            },
        )
    }
}

// A command lives in a window made ahead of time, so its size costs nothing at play time; boxing the note would
// mean freeing it on the audio thread.
#[allow(clippy::large_enum_variant)]
enum Kind {
    Map { key: u32, param: String, mapping: Option<Box<niminal_engine::ParamMapping>> },
    Buffer { buffer: Arc<niminal_engine::Buffer>, offset: usize, values: Arc<[f32]> },
    Note {
        track: Option<String>,
        program: Arc<Program>,
        off: u64,
        key: u32,
        voice: Option<Voice>,
        midi: Option<niminal_lang::MidiNote>,
    },
    /// Let go of the notes a key is holding open.
    Release { key: u32 },
    /// Move a parameter, by its index, of the notes a key is holding, over `frames`.
    SetParam {
        key: u32,
        name: String,
        value: f32,
        frames: u64,
    },
    /// A clip lane setting a track parameter: control key, value, glide in frames.
    Control {
        key: String,
        value: f32,
        frames: u64,
    },
    Swap {
        applied: Option<Arc<AtomicU64>>,
        mixer: Box<Mixer>,
        program: Arc<Program>,
        controls: Vec<ControlState>,
        transfers: Vec<Transfer>,
    },
    Reset {
        applied: Option<Arc<AtomicU64>>,
        mixer: Box<Mixer>,
        program: Arc<Program>,
        controls: Vec<ControlState>,
        epoch: u64,
    },
}

/// The command that moves a parameter of a held note, if `program` has the instrument and the parameter.
fn set_command(program: &Program, set: &super::VoiceSet, at: u64, frames: u64) -> Option<Command> {
    let _ = program;
    Some(Command { at, kind: Kind::SetParam { key: set.key, name: set.name.clone(), value: set.value, frames } })
}

fn map_command(program: &Program, set: &super::VoiceMap, at: u64) -> Command {
    Command { at, kind: Kind::Map { key: set.key, param: set.param.clone(), mapping: super::parameters::prepare_mapping(program,set) } }
}

/// A `midi_out` note on or off, if anyone is listening. Allocates nothing.
fn send_midi(
    tx: &mut Option<Producer<crate::MidiEvent>>,
    wake: &Option<std::thread::Thread>,
    at: u64,
    on: bool,
    note: &niminal_lang::MidiNote,
) {
    if let Some(tx) = tx {
        let sent = tx.push(crate::MidiEvent {
            at,
            on,
            port: note.port.clone(),
            channel: note.channel,
            note: note.note,
            velocity: note.velocity,
        }).is_ok();
        if sent && let Some(thread) = wake { thread.unpark(); }
    }
}

#[allow(clippy::large_enum_variant)] // boxing would allocate on the audio thread
enum Retired {
    Window(Box<Window>),
    Voice(Voice),
    Injected(Box<Injected>),
}

/// A command prepared for a stretch that has been prepared already, to be merged into it by sample.
/// It belongs to one revision, as a window does, and is dropped with it.
struct Injected {
    revision: u64,
    command: Command,
}

/// Something added since the planner last looked.
pub(super) enum Fresh {
    Note { event: Event, start: u64, dur: u64, key: Option<u32> },
    Release { key: u32, at: u64 },
    Set(super::VoiceSet),
    Track(super::TrackSet),
    Buffer(super::BufferSet),
    Map(super::VoiceMap),
}

impl Fresh {
    fn at(&self) -> u64 {
        match self {
            Fresh::Note { start, .. } => *start,
            Fresh::Release { at, .. } => *at,
            Fresh::Set(set) => set.at,
            Fresh::Track(set) => set.at,
            Fresh::Buffer(set) => set.at,
            Fresh::Map(set) => set.at,
        }
    }
}

pub(super) struct Planner {
    ready: Producer<Box<Window>>,
    inject_tx: Producer<Box<Injected>>,
    control_tx: Producer<Change>,
    control_applied: Consumer<Applied>,
    retired: Consumer<Retired>,
    pub(super) state: Arc<State>,
    config: RealtimeConfig,
    prepared_until: u64,
    revision: u64,
    swap_epoch: u64,
    reported: RenderMetrics,
    refresh: bool,
    replan: bool,
    deferred: Vec<Command>,
    stopped: Vec<String>,
}

impl Planner {
    pub(super) fn lookahead(&self) -> u64 {
        self.config.lookahead_frames
    }

    pub(super) fn push_control(&mut self, change: Change) -> Result<(), ()> {
        self.control_tx.push(change).map_err(|_| ())
    }
}

/// The render thread owns all running DSP state. Construction, pattern queries,
/// graph compilation, and resource destruction never take place in `render`.
pub struct RealtimeRenderer {
    ready: Consumer<Box<Window>>,
    inject_rx: Consumer<Box<Injected>>,
    /// Commands for stretches prepared already, in the order they run.
    #[allow(clippy::vec_box)] // moved to the retirement queue whole, without copying
    injected: Vec<Box<Injected>>,
    control_rx: Consumer<Change>,
    control_applied: Producer<Applied>,
    control_pending: Vec<Change>,
    controls: Vec<ControlState>,
    retired: Producer<Retired>,
    state: Arc<State>,
    panic_epoch: Arc<AtomicU64>,
    seen_panic: u64,
    mixer: Box<Mixer>,
    program: Arc<Program>,
    limiter: Option<Master>,
    channels: usize,
    sample_rate: f32,
    clock: u64,
    current: Option<Box<Window>>,
    cursor: usize,
    sounding: Sounding,
    finished: Vec<Voice>,
    config: RealtimeConfig,
    /// Audio from a device, for tracks that read `input(..)`.
    audio_in: Option<crate::AudioInput>,
    /// Where notes for `midi_out` instruments go, and those still sounding: when they end and the key that holds them.
    midi_tx: Option<Producer<crate::MidiEvent>>,
    midi_wake: Option<std::thread::Thread>,
}

impl Session {
    /// Detach running DSP once. The returned renderer is moved to its own
    /// thread; this session remains the authoritative control and score model.
    pub fn start_realtime(&mut self, config: RealtimeConfig) -> RealtimeRenderer {
        assert!(!self.detached, "a session has one renderer");
        assert!(config.window_frames > 0 && config.queued_windows > 0 && config.max_voices > 0);
        assert!(config.lookahead_frames > 0);
        for change in &mut self.control_changes {
            change.applied = false;
        }
        let (applied_tx, applied_rx) = RingBuffer::new(controls::CAPACITY);
        let (control_tx, control_rx) = RingBuffer::new(controls::CAPACITY);
        let (ready_tx, ready_rx) = RingBuffer::new(config.queued_windows);
        let (inject_tx, inject_rx) = RingBuffer::new(config.max_voices * 4);
        let (retired_tx, retired_rx) =
            RingBuffer::new(config.max_voices * 4 + config.queued_windows * 2);
        let state = Arc::new(State {
            clock: AtomicU64::new(self.clock),
            revision: AtomicU64::new(0),
            swap_epoch: AtomicU64::new(0),
            reset_epoch: AtomicU64::new(self.panic_epoch.load(Ordering::Acquire)),
            voices: AtomicUsize::new(self.mixer.active_voices()),
            peaks: std::array::from_fn(|_| AtomicU32::new(0)),
            silenced: AtomicUsize::new(0),
            late_events: AtomicUsize::new(0),
            starved_frames: AtomicUsize::new(0),
            capacity_drops: AtomicUsize::new(0),
            control_drops: AtomicUsize::new(0),
            retire_pressure: AtomicUsize::new(0),
            deadline_misses: AtomicUsize::new(0),
            planner_panics: AtomicUsize::new(0),
            max_render_micros: AtomicU64::new(0),
            latency: self.latency(),
            mixer_builds: self.mixer_builds.clone(),
            planner_thread: std::sync::Mutex::new(None),
        });
        assert!(
            self.mixer.active_voices() <= config.max_voices * 2,
            "existing voices exceed realtime capacity"
        );
        let mut mixer = std::mem::replace(&mut self.mixer, self.program.mixer(self.sample_rate));
        mixer.prepare_realtime(config.max_voices * 2);
        mixer.set_max_voices(config.max_voices);
        let mut sounding = std::mem::take(&mut self.sounding);
        sounding.held.reserve(config.max_voices * 2);
        sounding.midi.reserve(config.max_voices);
        let mut renderer = RealtimeRenderer {
            ready: ready_rx,
            inject_rx,
            injected: Vec::with_capacity(config.max_voices * 4),
            control_rx,
            control_applied: applied_tx,
            control_pending: self.control_changes.clone(),
            controls: self.controls.clone(),
            retired: retired_tx,
            state: state.clone(),
            panic_epoch: self.panic_epoch.clone(),
            seen_panic: self.panic_epoch.load(Ordering::Acquire),
            mixer: Box::new(mixer),
            program: self.program.clone(),
            limiter: self.limiter.take(),
            channels: self.channels(),
            sample_rate: self.sample_rate,
            clock: self.clock,
            current: None,
            cursor: 0,
            sounding,
            finished: Vec::with_capacity(config.max_voices * 2),
            config,
            audio_in: None,
            midi_tx: None,
            midi_wake: None,
        };
        renderer.control_pending.reserve(controls::CAPACITY);
        self.live = Some(Planner {
            ready: ready_tx,
            inject_tx,
            control_tx,
            control_applied: applied_rx,
            retired: retired_rx,
            state,
            config,
            prepared_until: self.clock,
            revision: 0,
            swap_epoch: 0,
            reported: RenderMetrics::default(),
            refresh: false,
            replan: false,
            deferred: Vec::new(),
            stopped: Vec::new(),
        });
        self.detached = true;
        self.prepare_realtime();
        renderer
    }

    /// Everything prepared is out of date: it is thrown away and prepared again.
    pub(super) fn invalidate_live(&self) {
        if let Some(planner) = &self.live {
            planner.state.swap_epoch.fetch_add(1, Ordering::AcqRel);
            planner.state.revision.fetch_add(1, Ordering::AcqRel);
        }
        self.wake_planner();
    }

    /// Something waiting was taken back: what is prepared is thrown away and prepared again, with the
    /// mixer as it is.
    pub(super) fn invalidate_windows(&self) {
        if let Some(planner) = &self.live {
            planner.state.revision.fetch_add(1, Ordering::AcqRel);
        }
        self.wake_planner();
    }

    /// Ask the planner to look at what was added, without throwing away what it has prepared.
    pub(super) fn wake_planner(&self) {
        if let Some(planner) = &self.live
            && let Some(thread) = planner
                .state
                .planner_thread
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
        {
            thread.unpark();
        }
    }

    /// The thread that calls [`Session::prepare_realtime`]: it is woken at once when a change lands,
    /// instead of when it next looks.
    pub fn set_planner_thread(&self, thread: std::thread::Thread) {
        if let Some(planner) = &self.live {
            *planner
                .state
                .planner_thread
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(thread);
        }
    }

    /// Bring control metadata up to the renderer's clock. This does no DSP.
    pub fn sync_realtime(&mut self) {
        let Some(planner) = &mut self.live else {
            return;
        };
        while let Ok(applied) = planner.control_applied.pop() {
            if let Some(entry) = self.log.get_mut(applied.log_index)
                && let LogInput::Send { ops, .. } = &mut entry.input
                && let Some(Op::Control { at, .. } | Op::Track { at, .. }) = ops.get_mut(applied.log_op)
            {
                if *at != At::Sample(applied.at) {
                    self.log_corrections += 1;
                }
                *at = At::Sample(applied.at);
            }
            if let Some(change) = self
                .control_changes
                .iter_mut()
                .find(|c| c.serial == applied.serial)
            {
                change.at = applied.at;
                change.applied = true;
            }
        }
        self.control_changes
            .sort_unstable_by_key(|c| (c.at, c.serial));
        let now = planner.state.clock.load(Ordering::Acquire);
        // Metadata can reach a graph boundary before DSP receives its prepared window.
        // Refresh once when consuming such a change, so it cannot disappear from planning.
        let unapplied_graph = self.pending.iter().any(|p| p.lands_at < now && p.applied.load(Ordering::Acquire) == super::transactions::WAITING);
        self.silenced += planner.state.silenced.swap(0, Ordering::Relaxed);
        for (peak, value) in self.peaks.iter_mut().zip(&planner.state.peaks) {
            *peak = peak.max(f32::from_bits(value.swap(0, Ordering::Relaxed)));
        }
        while self.clock < now {
            let mut next = self
                .pending
                .first()
                .map_or(now, |p| p.lands_at.max(self.clock).min(now));
            if let Some(change) = self.control_changes.iter().find(|c| c.applied) {
                next = next.min(change.at.max(self.clock));
            }
            if let Some(set) = self.track_sets.iter().find(|s| !s.done) && set.at < now {
                next = next.min(set.at.max(self.clock));
            }
            self.clock = next;
            self.land_due(now);
            self.apply_controls();
            self.apply_track_sets(if self.clock<now {self.clock.saturating_add(1)} else {self.clock});
        }
        self.land_due(now);
        self.apply_controls();
        self.apply_track_sets(self.clock);
        if unapplied_graph { self.invalidate_live(); }
        for set in &mut self.buffer_sets { if set.at < self.clock { set.done = true; } }
        let stale = self.clock.saturating_sub(self.sample_rate as u64 * 2);
        self.buffer_sets.retain(|s| !s.done || s.at > stale);
        for set in &mut self.voice_maps { if set.at < self.clock { set.done = true; } }
        self.voice_maps.retain(|s| !s.done || s.at > stale);
        let Some(planner) = &self.live else { return };
        if planner.revision == planner.state.revision.load(Ordering::Acquire) {
            let consumed = self.clock.min(planner.prepared_until);
            self.upcoming.retain(|n| n.start >= consumed);
        }
    }

    /// Plan separate from audio buffering. Every window owns constructed voices
    /// and ready mixers, including changes that land inside the lookahead.
    pub fn prepare_realtime(&mut self) {
        self.prepare_realtime_step(usize::MAX);
    }

    /// Like [`Session::prepare_realtime`], but plans at most `max_windows`. Returns whether more is waiting
    /// to be planned, so that a thread sharing the session can let others in between.
    pub fn prepare_realtime_step(&mut self, max_windows: usize) -> bool {
        self.sync_realtime();
        let Some(mut planner) = self.live.take() else {
            return false;
        };
        while let Ok(retired) = planner.retired.pop() {
            match retired {
                Retired::Window(window) => drop(window),
                Retired::Voice(voice) => drop(voice),
                Retired::Injected(injected) => drop(injected),
            }
        }
        let revision = planner.state.revision.load(Ordering::Acquire);
        if revision != planner.revision {
            let swap_epoch = planner.state.swap_epoch.load(Ordering::Acquire);
            planner.refresh |= swap_epoch != planner.swap_epoch;
            planner.swap_epoch = swap_epoch;
            planner.replan = true;
            planner.deferred.clear();
            planner.stopped.clear();
            planner.revision = revision;
            planner.prepared_until = self.clock;
            // the windows to come will take in everything that was added
            self.fresh.clear();
        }
        planner.prepared_until = planner.prepared_until.max(self.clock);
        self.inject_fresh(&mut planner);
        // The planner must survive a panic in planning, or the renderer starves for good: start clean instead.
        let planned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.plan_ahead(&mut planner, revision, max_windows)
        }));
        let more = planned.as_ref().is_ok_and(|more| *more);
        if planned.is_err() {
            planner.state.planner_panics.fetch_add(1, Ordering::Relaxed);
            self.restart_realtime();
        }
        let metrics = planner.state.metrics();
        if metrics.late_events > planner.reported.late_events {
            self.notify(format!(
                "{} prepared changes arrived after their sample deadline",
                metrics.late_events - planner.reported.late_events
            ));
        }
        if metrics.starved_frames > planner.reported.starved_frames {
            self.notify(format!(
                "event preparation fell behind for {} frames",
                metrics.starved_frames - planner.reported.starved_frames
            ));
        }
        if metrics.capacity_drops > planner.reported.capacity_drops {
            self.notify(format!(
                "{} notes exceeded the reserved live voice capacity",
                metrics.capacity_drops - planner.reported.capacity_drops
            ));
        }
        if metrics.control_drops > planner.reported.control_drops {
            self.notify(
                "queued control updates were dropped after their definitions changed".into(),
            );
        }
        if metrics.retire_pressure > planner.reported.retire_pressure {
            self.notify("the retirement queue is full; prepared changes are waiting".into());
        }
        if metrics.deadline_misses > planner.reported.deadline_misses {
            self.notify(format!(
                "DSP missed {} render deadlines",
                metrics.deadline_misses - planner.reported.deadline_misses
            ));
        }
        if metrics.planner_panics > planner.reported.planner_panics {
            self.notify("event preparation failed and was restarted; the sound was reset".into());
        }
        planner.reported = metrics;
        self.live = Some(planner);
        more
    }

    /// Ask the renderer to drop what it is playing and take a clean mixer from the next window.
    pub fn restart_realtime(&self) {
        self.panic_epoch.fetch_add(1, Ordering::Release);
        self.invalidate_live();
    }

    /// Prepare windows until the lookahead is full, the queue is, or `max_windows` are made.
    /// Returns whether it stopped for the last reason with more to do.
    fn plan_ahead(&mut self, planner: &mut Planner, revision: u64, max_windows: usize) -> bool {
        let horizon = self.clock.saturating_add(planner.config.lookahead_frames);
        let mut made = 0;
        while planner.prepared_until < horizon && planner.ready.slots() > 0 {
            if made == max_windows {
                return true;
            }
            made += 1;
            let from = planner.prepared_until;
            let to = (from + planner.config.window_frames as u64).min(horizon);
            let mode = if from == self.clock
                && planner.state.reset_epoch.load(Ordering::Acquire)
                    != self.panic_epoch.load(Ordering::Acquire)
            {
                PlanMode::Reset
            } else if planner.refresh {
                PlanMode::Refresh
            } else if planner.replan {
                PlanMode::Replan
            } else {
                PlanMode::Advance
            };
            let mut window = self.plan_window(
                from..to,
                revision,
                planner.config,
                mode,
                &mut planner.stopped,
            );

            planner
                .state
                .capacity_drops
                .fetch_add(window.dropped, Ordering::Relaxed);
            if let Some(at) = window
                .commands
                .iter()
                .filter(|c| matches!(c.kind, Kind::Reset { .. }))
                .map(|c| c.at)
                .min()
            {
                planner
                    .deferred
                    .retain(|c| c.at < at || !matches!(c.kind, Kind::Note { .. }));
            }
            let mut future = Vec::new();
            for command in planner
                .deferred
                .drain(..)
                .chain(std::mem::take(&mut window.commands))
            {
                if command.at < to {
                    window.commands.push(command);
                } else if future.len() < planner.config.max_voices * 4 {
                    future.push(command);
                } else {
                    drop(command); // Planner owns this allocation and destroys it here.
                    planner.state.capacity_drops.fetch_add(1, Ordering::Relaxed);
                }
            }
            planner.deferred = future;
            window.commands.sort_by_key(Command::order);
            match planner.ready.push(Box::new(window)) {
                Ok(()) => {
                    planner.prepared_until = to;
                    planner.refresh = false;
                    planner.replan = false;
                }
                Err(PushError::Full(window)) => {
                    drop(window);
                    break;
                }
            }
        }
        false
    }

    /// Put what was added since the planner last looked into the stretch already prepared, as commands the
    /// renderer merges in by sample. What starts after that stretch needs nothing: the windows still to be
    /// prepared take it from the session. If it can't be put in safely (a change of definitions or a panic is
    /// due before it, or the queue is full) the prepared windows are thrown away instead.
    fn inject_fresh(&mut self, planner: &mut Planner) {
        let fresh = std::mem::take(&mut self.fresh);
        let config = planner.config;
        for item in fresh {
            let at = item.at();
            if at >= planner.prepared_until {
                continue;
            }
            // a note is made with the program that is running when it starts
            let program = self.program_at(at).clone();
            let mut window = Window {
                end: planner.prepared_until,
                revision: planner.revision,
                commands: Vec::new(),
                notices: Vec::new(),
                stopped: Vec::new(),
                dropped: 0,
            };
            match item {
                Fresh::Note { event, start, dur, key, .. } => {
                    let hold = Hold { at: start.max(self.clock), dur, key };
                    self.prepare_note(&program, &event, hold, config, &mut window);
                }
                Fresh::Release { key, at } => window.commands.push(Command { at: at.max(self.clock), kind: Kind::Release { key } }),
                Fresh::Map(set) => window.commands.push(map_command(self.program_at(set.at),&set,set.at.max(self.clock))),
                Fresh::Buffer(set) => {
                    if let Some((_, buffer)) = self.program_at(set.at).buffers.iter().find(|(name, _)| *name == set.name)
                        && set.offset <= buffer.len() && set.values.len() <= buffer.len() - set.offset {
                        window.commands.push(Command { at: set.at.max(self.clock), kind: Kind::Buffer { buffer: buffer.clone(), offset: set.offset, values: set.values } });
                    }
                }
                Fresh::Track(set) => window.commands.push(Command {
                    at: set.at.max(self.clock),
                    kind: Kind::Control { key: set.key, value: set.value, frames: set.frames },
                }),
                Fresh::Set(set) => {
                    if let Some(command) = set_command(&program, &set, set.at.max(self.clock), set.frames) {
                        window.commands.push(command);
                    }
                }
            }
            let full = window.commands.len() > planner.inject_tx.slots();
            for notice in window.notices.drain(..) {
                self.notify(notice);
            }
            planner.state.capacity_drops.fetch_add(window.dropped, Ordering::Relaxed);
            if full {
                // the windows are made again, which the next step will see (the planner is out of the
                // session while it works, so this can't go through `invalidate_windows`)
                self.fresh.clear();
                planner.state.revision.fetch_add(1, Ordering::AcqRel);
                return;
            }
            for command in window.commands {
                let injected = Box::new(Injected { revision: planner.revision, command });
                let _ = planner.inject_tx.push(injected);
            }
        }
    }

    pub fn realtime_metrics(&self) -> Option<RenderMetrics> {
        self.live.as_ref().map(|p| p.state.metrics())
    }

    pub fn prepared_until(&self) -> u64 {
        self.live.as_ref().map_or(self.clock, |p| p.prepared_until)
    }

    fn plan_window(
        &mut self,
        range: std::ops::Range<u64>,
        revision: u64,
        config: RealtimeConfig,
        mode: PlanMode,
        stopped: &mut Vec<String>,
    ) -> Window {
        let (from, to) = (range.start, range.end);
        let mut window = Window {
            end: to,
            revision,
            commands: Vec::new(),
            notices: Vec::new(),
            stopped: std::mem::take(stopped),
            dropped: 0,
        };
        let mut program = self.program.clone();
        let explicit: Vec<Explicit> = self
            .upcoming
            .iter()
            .map(|n| Explicit {
                start: n.start.max(self.clock),
                dur: n.dur.saturating_sub(self.clock.saturating_sub(n.start)),
                event: n.event.clone(),
                key: n.key,
            })
            .collect();
        let mut position = from;
        match mode {
            PlanMode::Reset => {
                window.commands.push(self.reset_command(from, &program, config));
            }
            PlanMode::Refresh => {
                window.commands.push(self.swap_command(from, &program, config));
            }
            PlanMode::Replan | PlanMode::Advance => {}
        }
        // Pending changes are previewed without committing control state. Window boundaries and note times
        // remain sample positions, independent of how far ahead this thread happens to prepare them.

        let queue: Vec<(u64, &Pending)> = self.pending.iter().map(|p| (p.lands_at, p)).collect();
        let mut i = 0;
        while i < queue.len() {
            let (lands_at, pending) = queue[i];
            i += 1;
            if lands_at >= to {
                break;
            }
            let boundary = lands_at.max(from);
            if position < boundary {
                self.plan_notes(&program, &explicit, position..boundary, config, &mut window);
                position = boundary;
            }
            let next = &pending.program;
            if lands_at >= from {
                let mut swap = self.swap_command(lands_at, next, config);
                if let Kind::Swap { controls, applied, .. } = &mut swap.kind {
                    *applied=Some(pending.applied.clone());
                    controls::mark(controls, next, &pending.statements, pending.id);
                }
                window.commands.push(swap);
            }
            program = next.clone();
        }
        if position < to {
            self.plan_notes(&program, &explicit, position..to, config, &mut window);
        }
        // Stable sorting keeps a graph swap/reset before notes at the same sample.
        window.commands.sort_by_key(Command::order);
        for notice in window.notices.drain(..) {
            self.notify(notice);
        }
        *stopped = std::mem::take(&mut window.stopped);
        window
    }

    /// A mixer for `program`, built here so that the render thread need only swap it in.
    fn fresh_mixer(&self, program: &Program, config: RealtimeConfig) -> Box<Mixer> {
        self.mixer_builds.fetch_add(1, Ordering::Relaxed);
        let mut mixer = program.mixer(self.sample_rate);
        mixer.prepare_realtime(config.max_voices * 2);
        mixer.set_max_voices(config.max_voices);
        Box::new(mixer)
    }

    fn swap_command(&self, at: u64, program: &Arc<Program>, config: RealtimeConfig) -> Command {
        let mixer = self.fresh_mixer(program, config);
        let states = self.control_states(program);
        Command {
            at,
            kind: Kind::Swap {
                applied: Arc::ptr_eq(program, &self.program).then(|| self.current_transaction.clone()).flatten(),
                mixer,
                program: program.clone(),
                controls: states,
                transfers: Vec::with_capacity(program.tracks.len()),
            },
        }
    }

    fn control_states(&self, program: &Arc<Program>) -> Vec<ControlState> {
        let mut states = controls::build(program);
        if Arc::ptr_eq(program, &self.program) {
            for state in &mut states {
                if let Some(current) = self.controls.iter().find(|c| c.key == state.key) {
                    state.revision = current.revision;
                }
            }
        }
        states
    }

    fn reset_command(&self, at: u64, program: &Arc<Program>, config: RealtimeConfig) -> Command {
        let mixer = self.fresh_mixer(program, config);
        Command {
            at,
            kind: Kind::Reset {
                applied: Arc::ptr_eq(program, &self.program).then(|| self.current_transaction.clone()).flatten(),
                mixer,
                program: program.clone(),
                controls: self.control_states(program),
                epoch: self.panic_epoch.load(Ordering::Acquire),
            },
        }
    }

    fn plan_notes(
        &self,
        program: &Arc<Program>,
        explicit: &[Explicit],
        range: std::ops::Range<u64>,
        config: RealtimeConfig,
        window: &mut Window,
    ) {
        let (from, to) = (range.start, range.end);
        for panic in &self.timed_panics {
            if panic.at >= from && panic.at < to {
                window.commands.push(self.reset_command(panic.at, program, config));
            }
        }
        for note in explicit {
            if note.start >= from && note.start < to {
                self.prepare_note(
                    program,
                    &note.event,
                    Hold { at: note.start, dur: note.dur, key: note.key },
                    config,
                    window,
                );
            }
        }
        // changes to a held note's parameters: whatever was before this window has played
        for set in &self.voice_sets {
            if set.at >= from
                && set.at < to
                && let Some(command) = set_command(program, set, set.at, set.frames)
            {
                window.commands.push(command);
            }
        }
        for set in &self.voice_maps { if set.at >= from && set.at < to { window.commands.push(map_command(program,set,set.at)); } }
        for set in &self.buffer_sets {
            if set.at >= from && set.at < to
                && let Some((_, buffer)) = program.buffers.iter().find(|(name, _)| *name == set.name)
                && set.offset <= buffer.len() && set.values.len() <= buffer.len() - set.offset {
                window.commands.push(Command { at: set.at, kind: Kind::Buffer { buffer: buffer.clone(), offset: set.offset, values: set.values.clone() } });
            }
        }
        // track parameters moved by `track` events
        let mut moved = 0;
        for set in &self.track_sets {
            if set.at >= from && set.at < to {
                moved += 1;
                if moved > MAX_WINDOW_CONTROLS {
                    window.notice("an envelope lane changes a parameter too often; the extra changes were dropped");
                    break;
                }
                window.commands.push(Command {
                    at: set.at,
                    kind: Kind::Control { key: set.key.clone(), value: set.value, frames: set.frames },
                });
            }
        }
        // releases: each key is its own note, so a release that is repeated in a later window does no harm
        for off in &self.key_offs {
            if off.at < to {
                window.commands.push(Command { at: off.at.max(from), kind: Kind::Release { key: off.key } });
            }
        }
    }

    fn prepare_note(
        &self,
        program: &Arc<Program>,
        event: &Event,
        when: Hold,
        config: RealtimeConfig,
        window: &mut Window,
    ) {
        let Hold { at, dur, key } = when;
        if window.commands.len() >= config.max_voices * 4 {
            window.dropped += 1;
            window.notice("prepared event window reached capacity; extra notes were dropped");
            return;
        }
        let plan = match program.plan(event) {
            Ok(plan) => plan,
            Err(error) => {
                window
                    .notices
                    .push(format!("note for `{}`: {error}", event.target));
                return;
            }
        };
        {
            let mut voice = Voice::new(
                program.instruments[plan.instrument].graph.clone(),
                self.sample_rate,
            );
            for &(i, value) in &plan.params {
                voice.set_param(i, value);
            }
            voice.set_group(plan.choke);
            voice.set_rule(plan.rule);
            voice.set_pan(plan.pan);
            voice.set_dur(if key.is_some() && dur == 0 { 0.0 } else { dur as f32 / self.sample_rate });
            for (key, value, smooth) in &plan.controls {
                window.commands.push(Command {
                    at,
                    kind: Kind::Control {
                        key: key.clone(),
                        value: *value,
                        frames: (smooth * f64::from(self.sample_rate)).round() as u64,
                    },
                });
            }
            window.commands.push(Command {
                at,
                kind: Kind::Note {
                    track: (plan.track != 0).then(|| program.tracks[plan.track].name.clone()),
                    program: program.clone(),
                    off: if key.is_some() && dur == 0 {
                        u64::MAX
                    } else {
                        at.saturating_add(dur)
                    },
                    key: key.unwrap_or(super::NO_KEY),
                    voice: Some(voice),
                    midi: plan.midi,
                },
            });
        }
    }
}

/// When a note starts, how long it lasts, and the key holding it open if there is one.
struct Hold {
    at: u64,
    dur: u64,
    key: Option<u32>,
}

/// A note the engine was handed: when and how long, what, and the key holding it open.
struct Explicit {
    start: u64,
    dur: u64,
    event: Event,
    key: Option<u32>,
}

impl RealtimeRenderer {
    /// Send the notes of `midi_out` instruments down `tx`, stamped with the sample they belong to.
    pub fn set_midi_output(&mut self, tx: Producer<crate::MidiEvent>, wake: Option<std::thread::Thread>) {
        self.midi_tx = Some(tx);
        self.midi_wake = wake;
    }

    /// Every sounding `midi_out` note off, as `panic` and a reset do.
    fn all_midi_off(&mut self) {
        for (_, _, note) in self.sounding.midi.drain(..) {
            send_midi(&mut self.midi_tx, &self.midi_wake, self.clock, false, &note);
        }
    }

    /// The `midi_out` notes whose time is up go off.
    fn release_midi_due(&mut self) {
        let mut i = 0;
        while i < self.sounding.midi.len() {
            if self.sounding.midi[i].0 <= self.clock {
                let (_, _, note) = self.sounding.midi.swap_remove(i);
                send_midi(&mut self.midi_tx, &self.midi_wake, self.clock, false, &note);
            } else {
                i += 1;
            }
        }
    }

    /// Feed tracks that read `input(..)` from this device audio from now on.
    pub fn set_audio_input(&mut self, input: crate::AudioInput) {
        self.audio_in = Some(input);
    }

    pub fn clock(&self) -> u64 {
        self.clock
    }
    pub fn metrics(&self) -> RenderMetrics {
        self.state.metrics()
    }
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Invalidate buffered audio and request an off-thread clean mixer after
    /// the caller catches a DSP failure.
    pub fn request_panic(&self) {
        self.panic_epoch.fetch_add(1, Ordering::AcqRel);
        self.state.swap_epoch.fetch_add(1, Ordering::AcqRel);
        self.state.revision.fetch_add(1, Ordering::AcqRel);
    }

    fn retire_current(&mut self) -> bool {
        if self.current.is_none() {
            return true;
        }
        if self.retired.slots() == 0 {
            return false;
        }
        let Some(window) = self.current.take() else {
            return true;
        };
        match self.retired.push(Retired::Window(window)) {
            Ok(()) => {
                self.cursor = 0;
                true
            }
            Err(PushError::Full(Retired::Window(window))) => {
                self.current = Some(window);
                false
            }
            Err(_) => unreachable!(),
        }
    }

    fn return_voices(&mut self) {
        while !self.finished.is_empty() && self.retired.slots() > 0 {
            let Some(voice) = self.finished.pop() else {
                break;
            };
            match self.retired.push(Retired::Voice(voice)) {
                Ok(()) => {}
                Err(PushError::Full(Retired::Voice(voice))) => {
                    self.finished.push(voice);
                    break;
                }
                Err(_) => unreachable!(),
            }
        }
    }

    /// Take in the commands the planner added to stretches already prepared, drop those of an older
    /// revision, and carry out the ones that are due. Nothing here allocates or frees: finished
    /// commands go back to the planner thread to be destroyed.
    fn run_injected(&mut self, revision: u64) {
        while self.injected.len() < self.injected.capacity() {
            let Ok(injected) = self.inject_rx.pop() else { break };
            let order = injected.command.order();
            let place = self.injected.partition_point(|i| i.command.order() <= order);
            self.injected.insert(place, injected);
        }
        let mut i = 0;
        while i < self.injected.len() && self.retired.slots() > 0 {
            if self.injected[i].revision == revision {
                i += 1;
                continue;
            }
            let stale = self.injected.remove(i);
            let _ = self.retired.push(Retired::Injected(stale));
        }
        while self.retired.slots() > 0 {
            let Some(first) = self.injected.first() else { break };
            if first.command.at > self.clock {
                break;
            }
            let mut injected = self.injected.remove(0);
            if injected.command.at < self.clock {
                self.state.late_events.fetch_add(1, Ordering::Relaxed);
            }
            self.apply(&mut injected.command.kind);
            let _ = self.retired.push(Retired::Injected(injected));
        }
    }

    /// Carry out one prepared command at the current sample.
    fn apply(&mut self, kind: &mut Kind) {
        match kind {
            Kind::Buffer { buffer, offset, values } => {
                for (i, value) in values.iter().enumerate() { buffer.write(*offset + i, *value); }
            }
            Kind::Release { key } => {
                self.sounding.release_key(*key, self.clock);
            }
            Kind::Control { key, value, frames } => {
                if let Some(c) = self.controls.iter_mut().find(|c| c.key == *key) {
                    c.set(self.clock, *value, *frames);
                    self.mixer
                        .set_control(&c.key, c.start, c.target, c.frames, 0);
                }
            }
            Kind::Map { key, param, mapping } => {
                if let Some((_,id,_)) = self.sounding.held.iter().find(|h| h.2 == *key) {
                    self.mixer.swap_voice_mapping(*id,param,mapping);
                    crate::controls::sync_note(&self.controls,&mut self.mixer,*id,self.clock);
                }
            }
            Kind::SetParam { key, name, value, frames } => {
                for held in self.sounding.held.iter().filter(|h| h.2 == *key) { self.mixer.set_voice_param_glide(held.1,name,*value,*frames); }
            }
            Kind::Note {
                track,
                program,
                off,
                key,
                voice,
                midi,
            } => {
                if let Some(note) = midi.take()
                    && *off > self.clock
                    && self.sounding.midi.len() < self.sounding.midi.capacity()
                {
                    send_midi(&mut self.midi_tx, &self.midi_wake, self.clock, true, &note);
                    self.sounding.midi.push((*off, *key, note));
                }
                let target = track
                    .as_ref()
                    .map_or(Some(0), |name| self.program.track_index(name));
                if let Some(target) = target
                    && *off > self.clock
                    && self.sounding.held.len() < self.sounding.held.capacity()
                    && self.mixer.active_voices() < self.config.max_voices * 2
                    && let Some(mut prepared) = voice.take()
                {
                    prepared.remap_buses_with(&mut |i| {
                        program
                            .buses
                            .get(i)
                            .and_then(|name| self.program.buses.iter().position(|n| n == name))
                            .filter(|&j| program.bus_layouts[i] == self.program.bus_layouts[j])
                    });
                    controls::sync_voice(&self.controls, &mut prepared, self.clock);
                    if let Some(id) = self.mixer.merge_legato(target, &prepared) {
                        // the note took over a sounding voice; the spare goes back to be freed off this thread
                        self.sounding.held.push((*off, id, *key));
                        *voice = Some(prepared);
                    } else {
                        match self.mixer.note_prepared(target, prepared) {
                            Ok(id) => self.sounding.held.push((*off, id, *key)),
                            Err(rejected) => {
                                *voice = Some(rejected);
                                self.state.capacity_drops.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                } else {
                    self.state.capacity_drops.fetch_add(1, Ordering::Relaxed);
                }
            }
            Kind::Swap {
                applied,
                mixer,
                program,
                controls,
                transfers,
            } => {
                super::plan_transfers(&self.program, program, transfers);
                self.mixer.remap_buses_with(&mut |i| {
                    self.program
                        .buses
                        .get(i)
                        .and_then(|name| program.buses.iter().position(|n| n == name))
                        .filter(|&j| self.program.bus_layouts[i] == program.bus_layouts[j])
                });
                mixer.adopt(&mut self.mixer, transfers);
                crate::controls::adopt(
                    controls,
                    &self.controls,
                    program,
                    self.clock,
                    self.sample_rate,
                );
                crate::controls::sync_mixer(controls, mixer, self.clock);
                super::carry_buffers(&self.program, program);
                std::mem::swap(&mut self.controls, controls);
                std::mem::swap(&mut self.mixer, mixer);
                std::mem::swap(&mut self.program, program);
                if let Some(applied)=applied { let _=applied.compare_exchange(super::transactions::WAITING,self.clock,Ordering::AcqRel,Ordering::Acquire); }
            }
            Kind::Reset {
                applied,
                mixer,
                program,
                controls,
                epoch,
            } => {
                crate::controls::adopt(
                    controls,
                    &self.controls,
                    program,
                    self.clock,
                    self.sample_rate,
                );
                crate::controls::sync_mixer(controls, mixer, self.clock);
                std::mem::swap(&mut self.controls, controls);
                std::mem::swap(&mut self.mixer, mixer);
                std::mem::swap(&mut self.program, program);
                if let Some(limiter) = &mut self.limiter {
                    limiter.clear();
                }
                self.sounding.held.clear();
                for (_, _, note) in self.sounding.midi.drain(..) {
                    send_midi(&mut self.midi_tx, &self.midi_wake, self.clock, false, &note);
                }
                self.state.reset_epoch.store(*epoch, Ordering::Release);
                if let Some(applied)=applied { let _=applied.compare_exchange(super::transactions::WAITING,self.clock,Ordering::AcqRel,Ordering::Acquire); }
            }
        }
    }

    /// This entire path is bounded, lock-free, allocation-free and free of
    /// heap destruction, including voice completion, edits and panic.
    pub fn render(&mut self, frames: usize, sink: &mut dyn FnMut(&[[f32; BLOCK]], usize)) {
        let started = std::time::Instant::now();
        let _flush = niminal_engine::denormals::FlushToZero::new();
        let end = self.clock.saturating_add(frames as u64);
        let mut block = [[0.0; BLOCK]; MAX_CHANNELS];
        let mut stale_budget = self.config.queued_windows;
        while self.clock < end {
            self.return_voices();
            // a note that ends where another begins ends first
            self.release_midi_due();
            let waiting = self.control_pending.len();
            while self.control_pending.len() < controls::CAPACITY {
                let Ok(change) = self.control_rx.pop() else {
                    break;
                };
                if change.retract {
                    self.control_pending.retain(|c| c.serial != change.serial);
                } else {
                    self.control_pending.push(change);
                }
            }
            if self.control_pending.len() > waiting {
                self.control_pending
                    .sort_unstable_by_key(|c| (c.at, c.serial));
            }
            let panic = self.panic_epoch.load(Ordering::Acquire);
            if panic != self.seen_panic {
                self.seen_panic = panic;
                self.mixer.silence();
                if let Some(limiter) = &mut self.limiter {
                    limiter.clear();
                }
                self.sounding.held.clear();
                self.all_midi_off();
            }
            let revision = self.state.revision.load(Ordering::Acquire);
            loop {
                let done = self.current.as_ref().is_some_and(|w| {
                    w.revision != revision
                        || (self.cursor == w.commands.len() && w.end <= self.clock)
                });
                if done {
                    if stale_budget == 0 || !self.retire_current() {
                        break;
                    }
                    stale_budget -= 1;
                }
                if self.current.is_none() {
                    self.current = self.ready.pop().ok();
                    self.cursor = 0;
                    if self
                        .current
                        .as_ref()
                        .is_some_and(|w| w.revision != revision)
                    {
                        continue;
                    }
                }
                break;
            }
            let ready = self.current.as_ref().is_some_and(|w| {
                w.revision == revision && (w.end > self.clock || self.cursor < w.commands.len())
            });
            if ready {
                // taken out while its commands run, so that they can change the renderer
                if let Some(mut window) = self.current.take() {
                    while let Some(command) = window.commands.get_mut(self.cursor) {
                        if command.at > self.clock {
                            break;
                        }
                        if command.at < self.clock {
                            self.state.late_events.fetch_add(1, Ordering::Relaxed);
                        }
                        self.apply(&mut command.kind);
                        self.cursor += 1;
                    }
                    self.current = Some(window);
                }
            }
            self.run_injected(revision);
            while self.control_applied.slots() > 0
                && self
                    .control_pending
                    .first()
                    .is_some_and(|c| c.at <= self.clock)
            {
                let waiting = self.control_pending[0];
                if self
                    .controls
                    .get(waiting.index)
                    .is_none_or(|s| s.revision < waiting.revision)
                {
                    break; // Its accepted declaration has not reached DSP yet.
                }
                let change = self.control_pending.remove(0);
                let _ = self.control_applied.push(Applied {
                    serial: change.serial,
                    log_index: change.log_index,
                    log_op: change.log_op,
                    at: self.clock,
                });
                if change.at < self.clock {
                    self.state.late_events.fetch_add(1, Ordering::Relaxed);
                }
                if self
                    .program
                    .controls
                    .get(change.index)
                    .is_some_and(|d| d.unit == change.unit)
                    && let Some(control) = self.controls.get_mut(change.index)
                {
                    control.set(self.clock, change.value, change.frames);
                    self.mixer.set_control(
                        &control.key,
                        control.start,
                        control.target,
                        control.frames,
                        0,
                    );
                } else {
                    self.state.control_drops.fetch_add(1, Ordering::Relaxed);
                }
            }
            self.sounding.held.retain(|&(off, id, _)| {
                if off <= self.clock {
                    self.mixer.release(id);
                    false
                } else {
                    true
                }
            });
            self.release_midi_due();
            let mut n = (end - self.clock).min(BLOCK as u64);
            if ready && let Some(window) = &self.current {
                if let Some(command) = window.commands.get(self.cursor) {
                    n = n.min(command.at.saturating_sub(self.clock).max(1));
                }
                if window.end > self.clock {
                    n = n.min(window.end - self.clock);
                }
            }
            if let Some(next) = self.injected.first()
                && next.command.at > self.clock
            {
                n = n.min(next.command.at - self.clock);
            }
            if let Some(change) = self.control_pending.first()
                && change.at > self.clock
            {
                n = n.min(change.at - self.clock);
            }
            n = self.sounding.limit(self.clock, n);
            let n = n.max(1) as usize;
            if !ready {
                self.state.starved_frames.fetch_add(n, Ordering::Relaxed);
            }
            if let Some(input) = &mut self.audio_in {
                let mut heard = [[0.0; BLOCK]; MAX_CHANNELS];
                input.read(n, &mut heard);
                for (c, samples) in heard.iter().enumerate().take(input.channels()) {
                    self.mixer.set_audio_input(c, &samples[..n]);
                }
            }
            self.mixer.set_clock(niminal_engine::Clock { sample: self.clock });
            self.mixer
                .process_retained(&mut block[..self.channels], n, &mut self.finished);
            self.state
                .silenced
                .fetch_add(self.mixer.take_silenced(), Ordering::Relaxed);
            if let Some(limiter) = &mut self.limiter {
                limiter.process_block(&mut block[..self.channels], n);
            }
            for (peak, channel) in self.state.peaks.iter().zip(&block[..self.channels]) {
                let value = channel[..n].iter().fold(0.0f32, |m, s| m.max(s.abs()));
                peak.fetch_max(value.to_bits(), Ordering::Relaxed);
            }
            sink(&block[..self.channels], n);
            self.clock += n as u64;
            self.state.clock.store(self.clock, Ordering::Release);
            self.state
                .voices
                .store(self.mixer.active_voices(), Ordering::Relaxed);
        }
        self.return_voices();
        if self.retired.slots() == 0 {
            self.state.retire_pressure.fetch_add(1, Ordering::Relaxed);
        }
        let elapsed = started.elapsed();
        if frames > 0 {
            self.state
                .max_render_micros
                .fetch_max(elapsed.as_micros() as u64, Ordering::Relaxed);
            if elapsed.as_secs_f64() > frames as f64 / f64::from(self.sample_rate) {
                self.state.deadline_misses.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
