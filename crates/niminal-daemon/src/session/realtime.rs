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
    Refresh,
    Reset,
}

impl Default for RealtimeConfig {
    fn default() -> Self {
        Self {
            lookahead_frames: 8192,
            window_frames: 512,
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
}

pub(super) struct State {
    clock: AtomicU64,
    revision: AtomicU64,
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
                Kind::Swap { .. } | Kind::Control { .. } => 1,
                Kind::Note { .. } => 2,
                Kind::Release { .. } | Kind::Param { .. } => 3,
            },
        )
    }
}

// A command lives in a window made ahead of time, so its size costs nothing at play time; boxing the note would
// mean freeing it on the audio thread.
#[allow(clippy::large_enum_variant)]
enum Kind {
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
    /// Change a parameter (a keyboard's `bend` or `pressure`) of the notes a key is holding.
    Param {
        key: u32,
        name: &'static str,
        value: f32,
    },
    /// A clip lane setting a track parameter: control key, value, glide in frames.
    Control {
        key: String,
        value: f32,
        frames: u64,
    },
    Swap {
        mixer: Box<Mixer>,
        program: Arc<Program>,
        controls: Vec<ControlState>,
        transfers: Vec<Transfer>,
    },
    Reset {
        mixer: Box<Mixer>,
        program: Arc<Program>,
        controls: Vec<ControlState>,
        epoch: u64,
    },
}

/// A `midi_out` note on or off, if anyone is listening. Allocates nothing.
fn send_midi(
    tx: &mut Option<Producer<crate::MidiEvent>>,
    at: u64,
    on: bool,
    note: &niminal_lang::MidiNote,
) {
    if let Some(tx) = tx {
        let _ = tx.push(crate::MidiEvent {
            at,
            on,
            port: note.port.clone(),
            channel: note.channel,
            note: note.note,
            velocity: note.velocity,
        });
    }
}

enum Retired {
    Window(Box<Window>),
    Voice(Voice),
}

pub(super) struct Planner {
    ready: Producer<Box<Window>>,
    control_tx: Producer<Change>,
    control_applied: Consumer<Applied>,
    retired: Consumer<Retired>,
    pub(super) state: Arc<State>,
    config: RealtimeConfig,
    prepared_until: u64,
    revision: u64,
    reported: RenderMetrics,
    refresh: bool,
    deferred: Vec<Command>,
    stopped: Vec<String>,
}

impl Planner {
    pub(super) fn push_control(&mut self, change: Change) -> Result<(), ()> {
        self.control_tx.push(change).map_err(|_| ())
    }
}

/// The render thread owns all running DSP state. Construction, pattern queries,
/// graph compilation, and resource destruction never take place in `render`.
pub struct RealtimeRenderer {
    ready: Consumer<Box<Window>>,
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
        let (retired_tx, retired_rx) =
            RingBuffer::new(config.max_voices * 4 + config.queued_windows * 2);
        let state = Arc::new(State {
            clock: AtomicU64::new(self.clock),
            revision: AtomicU64::new(0),
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
        };
        renderer.control_pending.reserve(controls::CAPACITY);
        self.live = Some(Planner {
            ready: ready_tx,
            control_tx,
            control_applied: applied_rx,
            retired: retired_rx,
            state,
            config,
            prepared_until: self.clock,
            revision: 0,
            reported: RenderMetrics::default(),
            refresh: false,
            deferred: Vec::new(),
            stopped: Vec::new(),
        });
        self.detached = true;
        self.prepare_realtime();
        renderer
    }

    pub(super) fn invalidate_live(&self) {
        if let Some(planner) = &self.live {
            planner.state.revision.fetch_add(1, Ordering::AcqRel);
            if let Some(thread) = planner
                .state
                .planner_thread
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                thread.unpark();
            }
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
                && let LogInput::Control { at, .. } = &mut entry.input
            {
                if *at != applied.at {
                    self.log_corrections += 1;
                }
                *at = applied.at;
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
            self.clock = next;
            self.land_due();
            self.apply_controls();
        }
        self.land_due();
        self.apply_panics();
        self.apply_controls();
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
            }
        }
        let revision = planner.state.revision.load(Ordering::Acquire);
        if revision != planner.revision {
            planner.refresh = true;
            planner.deferred.clear();
            planner.stopped.clear();
            planner.revision = revision;
            planner.prepared_until = self.clock;
        }
        planner.prepared_until = planner.prepared_until.max(self.clock);
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
                }
                Err(PushError::Full(window)) => {
                    drop(window);
                    break;
                }
            }
        }
        false
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
        let mut history = self.history.clone();
        let mut schedule = schedule_for(&program, &history);
        let mut explicit: Vec<Explicit> = self
            .upcoming
            .iter()
            .filter(|n| !n.scheduled)
            .map(|n| Explicit {
                start: n.start.max(self.clock),
                dur: n.dur.saturating_sub(self.clock.saturating_sub(n.start)),
                event: n.event.clone(),
                created: self.clock,
                key: n.key,
            })
            .collect();
        let mut position = from;
        match mode {
            PlanMode::Reset => {
                window
                    .commands
                    .push(self.reset_command(from, &program, config));
            }
            PlanMode::Refresh => {
                window
                    .commands
                    .push(self.swap_command(from, &program, config));
            }
            PlanMode::Advance => {}
        }
        // Pending transactions are previewed without committing control state.
        // Window boundaries and note times remain sample positions, independent
        // of how far ahead this thread happens to prepare them.
        // A new tempo or meter moves the changes still waiting, as it will when it lands for real.
        let mut queue: Vec<(u64, &Pending)> =
            self.pending.iter().map(|p| (p.lands_at, p)).collect();
        let mut i = 0;
        while i < queue.len() {
            let (lands_at, pending) = queue[i];
            i += 1;
            if lands_at >= to {
                break;
            }
            let boundary = lands_at.max(from);
            if position < boundary {
                self.plan_notes(
                    &program,
                    &schedule,
                    &explicit,
                    position..boundary,
                    config,
                    &mut window,
                );
                position = boundary;
            }
            if let Some(next) = &pending.program {
                if lands_at >= from {
                    let mut swap = self.swap_command(lands_at, next, config);
                    if let Kind::Swap { controls, .. } = &mut swap.kind {
                        controls::mark(controls, next, &pending.statements, pending.id);
                    }
                    window.commands.push(swap);
                }
                let landing = self.seconds(lands_at);
                super::retime(&mut history, landing, program.tempo, next.tempo);
                if program.tempo != next.tempo {
                    for (at, p) in &mut queue[i..] {
                        if !super::is_timing(p) {
                            *at = super::rescale_landing(
                                *at,
                                lands_at,
                                super::landing_factor(
                                    self.landing_unit_is_bar(p),
                                    program.tempo,
                                    next.tempo,
                                ),
                            );
                        }
                    }
                    queue[i..].sort_by_key(|(at, _)| *at);
                }
                program = next.clone();
            }
            for action in pending.actions.iter().chain(&pending.replays) {
                preview_action(
                    action,
                    lands_at,
                    self.sample_rate,
                    &mut history,
                    &mut explicit,
                );
            }
            // Pending landings change the program and/or history the next slice plans against.
            schedule = schedule_for(&program, &history);
        }
        if position < to {
            self.plan_notes(
                &program,
                &schedule,
                &explicit,
                position..to,
                config,
                &mut window,
            );
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
        schedule: &Schedule,
        explicit: &[Explicit],
        range: std::ops::Range<u64>,
        config: RealtimeConfig,
        window: &mut Window,
    ) {
        let (from, to) = (range.start, range.end);
        let skip: Vec<_> = window
            .stopped
            .iter()
            .filter_map(|name| program.track_index(name))
            .collect();
        let found = schedule.events_lenient(self.seconds(from), self.seconds(to), &skip);
        if let Some(problem) = found.problem {
            window.notices.push(problem.to_string());
        }
        for i in found.overloaded {
            let name = program.tracks[i].name.clone();
            if !window.stopped.contains(&name) {
                window.stopped.push(name);
            }
        }
        for &at in schedule.panics() {
            let at = self.samples(at);
            if at >= from && at < to {
                window
                    .commands
                    .push(self.reset_command(at, program, config));
            }
        }
        // envelope lanes moving track parameters
        for set in found.controls.iter().take(MAX_WINDOW_CONTROLS) {
            let Some(def) = program.controls.iter().find(|c| c.name == set.name) else {
                continue;
            };
            let at = self.samples(set.at).max(from);
            let frames = (set.glide * f64::from(self.sample_rate)).round() as u64;
            window.commands.push(Command {
                at,
                kind: Kind::Control {
                    key: def.key.clone(),
                    value: set.target as f32,
                    frames,
                },
            });
        }
        if found.controls.len() > MAX_WINDOW_CONTROLS {
            window.notice(
                "an envelope lane changes a parameter too often; the extra changes were dropped",
            );
        }
        for event in found.events {
            let at = self.samples(event.at.to_seconds(program.tempo));
            let dur = self.samples(event.dur.to_seconds(program.tempo));
            self.prepare_note(
                program,
                &event,
                Hold {
                    at: at.max(from),
                    dur,
                    key: None,
                },
                config,
                window,
            );
        }
        for note in explicit {
            let panic = schedule
                .panics()
                .iter()
                .map(|&at| self.samples(at))
                .find(|&at| at >= note.created);
            if note.start >= from && note.start < to && panic.is_none_or(|p| note.start < p) {
                self.prepare_note(
                    program,
                    &note.event,
                    Hold {
                        at: note.start,
                        dur: note.dur,
                        key: note.key,
                    },
                    config,
                    window,
                );
            }
        }
        // a key's bend and pressure: those from this window as they were, and for each the latest from
        // before it, in case the window that carried it never played
        let mut latest: Vec<(u32, &'static str, f32)> = Vec::new();
        for &(key, name, value, at) in &self.key_params {
            if at >= from && at < to {
                window.commands.push(Command {
                    at,
                    kind: Kind::Param { key, name, value },
                });
            } else if at < from {
                match latest.iter_mut().find(|l| l.0 == key && l.1 == name) {
                    Some(l) => l.2 = value,
                    None => latest.push((key, name, value)),
                }
            }
        }
        for (key, name, value) in latest {
            window.commands.push(Command {
                at: from,
                kind: Kind::Param { key, name, value },
            });
        }
        // keys that came up: each key is its own note, so a release that is repeated in a later window does no harm
        for &(key, at) in &self.key_offs {
            if at < to {
                window.commands.push(Command {
                    at: at.max(from),
                    kind: Kind::Release { key },
                });
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
                    off: if key.is_some() {
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

/// A note the session was handed directly: when and how long, what, the clock it was handed at, and the key holding it open.
struct Explicit {
    start: u64,
    dur: u64,
    event: Event,
    created: u64,
    key: Option<u32>,
}

fn preview_action(
    action: &Compiled,
    at: u64,
    sample_rate: f32,
    history: &mut Vec<HistoryEntry>,
    explicit: &mut Vec<Explicit>,
) {
    let sample = |sec: f64| (sec * f64::from(sample_rate)).round().max(0.0) as u64;
    let origin = super::origin_of(history);
    for event in &action.program.notes {
        let offset = event.at.to_seconds(action.program.tempo);
        let start = if action.statement.has_at() {
            sample(origin + offset).max(at)
        } else {
            at + sample(offset)
        };
        explicit.push(Explicit {
            start,
            dur: sample(event.dur.to_seconds(action.program.tempo)),
            event: event.clone(),
            created: at,
            key: None,
        });
    }
    for scheduled in &action.program.performance {
        let after = scheduled.at.to_seconds(action.program.tempo);
        let when = if action.statement.has_at() {
            origin + after
        } else {
            at as f64 / f64::from(sample_rate) + after
        };
        history.push(HistoryEntry {
            at: when,
            action: name_action(&action.program, &scheduled.action),
        });
    }
}

pub(super) fn schedule_for(program: &Program, history: &[HistoryEntry]) -> Schedule {
    let names = program.tracks.iter().map(|t| t.name.clone()).collect();
    let index = |name: &String| program.track_index(name);
    let commands = history
        .iter()
        .filter_map(|h| {
            let action = match &h.action {
                NamedAction::Play { track, clip } => Action::Play {
                    track: index(track)?,
                    clip: clip.clone(),
                },
                NamedAction::Stop(t) => Action::Stop { track: index(t)? },
                NamedAction::Mute(t) => Action::Mute { track: index(t)? },
                NamedAction::Unmute(t) => Action::Unmute { track: index(t)? },
                NamedAction::Solo(t) => Action::Solo { track: index(t)? },
                NamedAction::Unsolo(t) => Action::Unsolo {
                    track: t.as_ref().and_then(index),
                },
                NamedAction::Hush => Action::Hush,
                NamedAction::Panic => Action::Panic,
                NamedAction::Reset => Action::Reset,
                NamedAction::Origin => return None,
            };
            Some(Scheduled {
                at: Time::Seconds(h.at),
                action,
                quantize: None,
            })
        })
        .collect::<Vec<_>>();
    Schedule::from_commands(program.tempo, names, &commands)
}

impl RealtimeRenderer {
    /// Send the notes of `midi_out` instruments down `tx`, stamped with the sample they belong to.
    pub fn set_midi_output(&mut self, tx: Producer<crate::MidiEvent>) {
        self.midi_tx = Some(tx);
    }

    /// Every sounding `midi_out` note off, as `panic` and a reset do.
    fn all_midi_off(&mut self) {
        for (_, _, note) in self.sounding.midi.drain(..) {
            send_midi(&mut self.midi_tx, self.clock, false, &note);
        }
    }

    /// The `midi_out` notes whose time is up go off.
    fn release_midi_due(&mut self) {
        let mut i = 0;
        while i < self.sounding.midi.len() {
            if self.sounding.midi[i].0 <= self.clock {
                let (_, _, note) = self.sounding.midi.swap_remove(i);
                send_midi(&mut self.midi_tx, self.clock, false, &note);
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

    /// Carry out one prepared command at the current sample.
    fn apply(&mut self, kind: &mut Kind) {
        match kind {
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
            Kind::Param { key, name, value } => {
                for held in self.sounding.held.iter().filter(|h| h.2 == *key) {
                    self.mixer.set_voice_param(held.1, name, *value);
                }
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
                    send_midi(&mut self.midi_tx, self.clock, true, &note);
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
                std::mem::swap(&mut self.controls, controls);
                std::mem::swap(&mut self.mixer, mixer);
                std::mem::swap(&mut self.program, program);
            }
            Kind::Reset {
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
                    send_midi(&mut self.midi_tx, self.clock, false, &note);
                }
                self.state.reset_epoch.store(*epoch, Ordering::Release);
            }
        }
    }

    /// This entire path is bounded, lock-free, allocation-free and free of
    /// heap destruction, including voice completion, edits and panic.
    pub fn render(&mut self, frames: usize, sink: &mut dyn FnMut(&[[f32; BLOCK]], usize)) {
        let started = std::time::Instant::now();
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
                self.control_pending.push(change);
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
