//! Prepared event windows into an exclusively owned DSP renderer. Both queues
//! are bounded: one delivers constructed voices and mixers, the other returns
//! their storage for destruction on the planning thread.

use super::*;
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

impl Default for RealtimeConfig {
    fn default() -> Self {
        Self { lookahead_frames: 8192, window_frames: 512, queued_windows: 32, max_voices: 512 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderMetrics {
    pub late_events: usize,
    pub starved_frames: usize,
    pub capacity_drops: usize,
    pub retire_pressure: usize,
    pub deadline_misses: usize,
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
    retire_pressure: AtomicUsize,
    deadline_misses: AtomicUsize,
    max_render_micros: AtomicU64,
    pub(super) latency: usize,
}

impl State {
    fn metrics(&self) -> RenderMetrics {
        RenderMetrics {
            late_events: self.late_events.load(Ordering::Relaxed),
            starved_frames: self.starved_frames.load(Ordering::Relaxed),
            capacity_drops: self.capacity_drops.load(Ordering::Relaxed),
            retire_pressure: self.retire_pressure.load(Ordering::Relaxed),
            deadline_misses: self.deadline_misses.load(Ordering::Relaxed),
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

struct Command { at: u64, kind: Kind }

enum Kind {
    Note { track: Option<String>, program: Arc<Program>, off: u64, voice: Option<Voice> },
    Swap { mixer: Box<Mixer>, program: Arc<Program>, transfers: Vec<Transfer> },
    Reset { mixer: Box<Mixer>, program: Arc<Program>, epoch: u64 },
}

enum Retired {
    Window(Box<Window>),
    Voice(Voice),
}

pub(super) struct Planner {
    ready: Producer<Box<Window>>,
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

/// The render thread owns all running DSP state. Construction, pattern queries,
/// graph compilation, and resource destruction never take place in `render`.
pub struct RealtimeRenderer {
    ready: Consumer<Box<Window>>,
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
    held: Vec<(u64, VoiceId)>,
    finished: Vec<Voice>,
    config: RealtimeConfig,
}

impl Session {
    /// Detach running DSP once. The returned renderer is moved to its own
    /// thread; this session remains the authoritative control and score model.
    pub fn start_realtime(&mut self, config: RealtimeConfig) -> RealtimeRenderer {
        assert!(!self.detached, "a session has one renderer");
        assert!(config.window_frames > 0 && config.queued_windows > 0 && config.max_voices > 0);
        assert!(config.lookahead_frames > 0);
        let (ready_tx, ready_rx) = RingBuffer::new(config.queued_windows);
        let (retired_tx, retired_rx) = RingBuffer::new(config.max_voices * 4 + config.queued_windows * 2);
        let state = Arc::new(State {
            clock: AtomicU64::new(self.clock), revision: AtomicU64::new(0),
            reset_epoch: AtomicU64::new(self.panic_epoch.load(Ordering::Acquire)),
            voices: AtomicUsize::new(self.mixer.active_voices()), peaks: std::array::from_fn(|_| AtomicU32::new(0)),
            silenced: AtomicUsize::new(0), late_events: AtomicUsize::new(0), starved_frames: AtomicUsize::new(0),
            capacity_drops: AtomicUsize::new(0), retire_pressure: AtomicUsize::new(0), deadline_misses: AtomicUsize::new(0), max_render_micros: AtomicU64::new(0), latency: self.latency(),
        });
        assert!(self.mixer.active_voices() <= config.max_voices * 2, "existing voices exceed realtime capacity");
        let mut mixer = std::mem::replace(&mut self.mixer, self.program.mixer(self.sample_rate));
        mixer.prepare_realtime(config.max_voices * 2);
        mixer.set_max_voices(config.max_voices);
        let mut held = std::mem::take(&mut self.held);
        held.reserve(config.max_voices * 2);
        let renderer = RealtimeRenderer {
            ready: ready_rx, retired: retired_tx, state: state.clone(), panic_epoch: self.panic_epoch.clone(),
            seen_panic: self.panic_epoch.load(Ordering::Acquire), mixer: Box::new(mixer), program: self.program.clone(),
            limiter: self.limiter.take(), channels: self.channels(), sample_rate: self.sample_rate, clock: self.clock,
            current: None, cursor: 0, held, finished: Vec::with_capacity(config.max_voices * 2), config,
        };
        self.live = Some(Planner { ready: ready_tx, retired: retired_rx, state, config,
            prepared_until: self.clock, revision: 0, reported: RenderMetrics::default(), refresh: false, deferred: Vec::new(), stopped: Vec::new() });
        self.detached = true;
        self.prepare_realtime();
        renderer
    }

    pub(super) fn invalidate_live(&self) {
        if let Some(planner) = &self.live { planner.state.revision.fetch_add(1, Ordering::AcqRel); }
    }

    /// Bring control metadata up to the renderer's clock. This does no DSP.
    pub fn sync_realtime(&mut self) {
        let Some(planner) = &self.live else { return };
        let now = planner.state.clock.load(Ordering::Acquire);
        self.silenced += planner.state.silenced.swap(0, Ordering::Relaxed);
        for (peak, value) in self.peaks.iter_mut().zip(&planner.state.peaks) {
            *peak = peak.max(f32::from_bits(value.swap(0, Ordering::Relaxed)));
        }
        while self.clock < now {
            self.clock = self.pending.first().map_or(now, |p| p.lands_at.max(self.clock).min(now));
            self.land_due();
        }
        self.land_due();
        self.apply_panics();
        let planner = self.live.as_ref().unwrap();
        if planner.revision == planner.state.revision.load(Ordering::Acquire) {
            let consumed = self.clock.min(planner.prepared_until);
            self.upcoming.retain(|n| n.start >= consumed);
        }
    }

    /// Plan separate from audio buffering. Every window owns constructed voices
    /// and ready mixers, including changes that land inside the lookahead.
    pub fn prepare_realtime(&mut self) {
        self.sync_realtime();
        let Some(mut planner) = self.live.take() else { return };
        while let Ok(retired) = planner.retired.pop() {
            match retired { Retired::Window(window) => drop(window), Retired::Voice(voice) => drop(voice) }
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
        let horizon = self.clock.saturating_add(planner.config.lookahead_frames);
        while planner.prepared_until < horizon && planner.ready.slots() > 0 {
            let from = planner.prepared_until;
            let to = (from + planner.config.window_frames as u64).min(horizon);
            let reset = from == self.clock
                && planner.state.reset_epoch.load(Ordering::Acquire) != self.panic_epoch.load(Ordering::Acquire);
            let mut window = self.plan_window(from..to, revision, planner.config, reset, planner.refresh, &mut planner.stopped);

            planner.state.capacity_drops.fetch_add(window.dropped, Ordering::Relaxed);
            if let Some(at) = window.commands.iter().filter(|c| matches!(c.kind, Kind::Reset { .. })).map(|c| c.at).min() {
                planner.deferred.retain(|c| c.at < at || !matches!(c.kind, Kind::Note { .. }));
            }
            let mut future = Vec::new();
            for command in planner.deferred.drain(..).chain(std::mem::take(&mut window.commands)) {
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
            window.commands.sort_by_key(|c| (c.at, match c.kind {
                Kind::Reset { .. } => 0, Kind::Swap { .. } => 1, Kind::Note { .. } => 2,
            }));
            match planner.ready.push(Box::new(window)) {
                Ok(()) => { planner.prepared_until = to; planner.refresh = false; },
                Err(PushError::Full(window)) => { drop(window); break; }
            }
        }
        let metrics = planner.state.metrics();
        if metrics.late_events > planner.reported.late_events {
            self.notify(format!("{} prepared changes arrived after their sample deadline", metrics.late_events - planner.reported.late_events));
        }
        if metrics.starved_frames > planner.reported.starved_frames {
            self.notify(format!("event preparation fell behind for {} frames", metrics.starved_frames - planner.reported.starved_frames));
        }
        if metrics.capacity_drops > planner.reported.capacity_drops {
            self.notify(format!("{} notes exceeded the reserved live voice capacity", metrics.capacity_drops - planner.reported.capacity_drops));
        }
        if metrics.retire_pressure > planner.reported.retire_pressure {
            self.notify("the retirement queue is full; prepared changes are waiting".into());
        }
        if metrics.deadline_misses > planner.reported.deadline_misses {
            self.notify(format!("DSP missed {} render deadlines", metrics.deadline_misses - planner.reported.deadline_misses));
        }
        planner.reported = metrics;
        self.live = Some(planner);
    }

    pub fn realtime_metrics(&self) -> Option<RenderMetrics> {
        self.live.as_ref().map(|p| p.state.metrics())
    }

    pub fn prepared_until(&self) -> u64 {
        self.live.as_ref().map_or(self.clock, |p| p.prepared_until)
    }

    fn plan_window(&mut self, range: std::ops::Range<u64>, revision: u64, config: RealtimeConfig, reset: bool, refresh: bool, stopped: &mut Vec<String>) -> Window {
        let (from, to) = (range.start, range.end);
        let mut window = Window { end: to, revision, commands: Vec::new(), notices: Vec::new(), stopped: std::mem::take(stopped), dropped: 0 };
        let mut program = self.program.clone();
        let mut history = self.history.clone();
        let mut explicit: Vec<(u64, u64, Event, u64)> = self.upcoming.iter()
            .filter(|n| !n.scheduled).map(|n| (n.start.max(self.clock), n.dur.saturating_sub(self.clock.saturating_sub(n.start)), n.event.clone(), self.clock)).collect();
        let mut position = from;
        if reset { window.commands.push(self.reset_command(from, &program, config)); }
        else if refresh { window.commands.push(self.swap_command(from, &program, config)); }
        // Pending transactions are previewed without committing control state.
        // Window boundaries and note times remain sample positions, independent
        // of how far ahead this thread happens to prepare them.
        for pending in &self.pending {
            if pending.lands_at >= to { break; }
            let boundary = pending.lands_at.max(from);
            if position < boundary {
                self.plan_notes(&program, &history, &explicit, position..boundary, config, &mut window);
                position = boundary;
            }
            if let Some(next) = &pending.program {
                if pending.lands_at >= from {
                    window.commands.push(self.swap_command(pending.lands_at, next, config));
                }
                program = next.clone();
            }
            for action in pending.actions.iter().chain(&pending.replays) {
                preview_action(action, pending.lands_at, self.sample_rate, &mut history, &mut explicit);
            }
        }
        if position < to { self.plan_notes(&program, &history, &explicit, position..to, config, &mut window); }
        // Stable sorting keeps a graph swap/reset before notes at the same sample.
        window.commands.sort_by_key(|c| (c.at, match c.kind {
                Kind::Reset { .. } => 0, Kind::Swap { .. } => 1, Kind::Note { .. } => 2,
            }));
        for notice in window.notices.drain(..) { self.notify(notice); }
        *stopped = std::mem::take(&mut window.stopped);
        window
    }

    fn swap_command(&self, at: u64, program: &Arc<Program>, config: RealtimeConfig) -> Command {
        let mut mixer = program.mixer(self.sample_rate);
        mixer.prepare_realtime(config.max_voices * 2);
        mixer.set_max_voices(config.max_voices);
        Command { at, kind: Kind::Swap { mixer: Box::new(mixer), program: program.clone(),
            transfers: Vec::with_capacity(program.tracks.len()) } }
    }

    fn reset_command(&self, at: u64, program: &Arc<Program>, config: RealtimeConfig) -> Command {
        let mut mixer = program.mixer(self.sample_rate);
        mixer.prepare_realtime(config.max_voices * 2);
        mixer.set_max_voices(config.max_voices);
        Command { at, kind: Kind::Reset { mixer: Box::new(mixer), program: program.clone(), epoch: self.panic_epoch.load(Ordering::Acquire) } }
    }

    fn plan_notes(&self, program: &Arc<Program>, history: &[HistoryEntry], explicit: &[(u64, u64, Event, u64)],
                  range: std::ops::Range<u64>, config: RealtimeConfig, window: &mut Window) {
        let (from, to) = (range.start, range.end);
        let schedule = schedule_for(program, history);
        let skip: Vec<_> = window.stopped.iter().filter_map(|name| program.track_index(name)).collect();
        let found = schedule.events_lenient(self.seconds(from), self.seconds(to), &skip);
        if let Some(problem) = found.problem { window.notices.push(problem.to_string()); }
        for i in found.overloaded {
            let name = program.tracks[i].name.clone();
            if !window.stopped.contains(&name) { window.stopped.push(name); }
        }
        for &at in schedule.panics() {
            let at = self.samples(at);
            if at >= from && at < to {
                window.commands.push(self.reset_command(at, program, config));
            }
        }
        for event in found.events {
            let at = self.samples(event.at.to_seconds(program.tempo));
            let dur = self.samples(event.dur.to_seconds(program.tempo));
            self.prepare_note(program, &event, at.max(from), dur, config, window);
        }
        for (at, dur, event, created) in explicit {
            let panic = schedule.panics().iter().map(|&at| self.samples(at)).find(|&at| at >= *created);
            if *at >= from && *at < to && panic.is_none_or(|p| *at < p) {
                self.prepare_note(program, event, *at, *dur, config, window);
            }
        }
    }

    fn prepare_note(&self, program: &Arc<Program>, event: &Event, at: u64, dur: u64, config: RealtimeConfig, window: &mut Window) {
        if window.commands.len() >= config.max_voices * 4 {
            window.dropped += 1;
            let notice = "prepared event window reached capacity; extra notes were dropped";
            if !window.notices.iter().any(|n| n == notice) { window.notices.push(notice.into()); }
            return;
        }
        let plan = match program.plan(event) {
            Ok(plan) => plan,
            Err(error) => { window.notices.push(format!("note for `{}`: {error}", event.target)); return; }
        };
        {
            let mut voice = Voice::new(program.instruments[plan.instrument].graph.clone(), self.sample_rate);
            for &(i, value) in &plan.params { voice.set_param(i, value); }
            voice.set_group(plan.choke);
            window.commands.push(Command { at, kind: Kind::Note { track: (plan.track != 0).then(|| program.tracks[plan.track].name.clone()),
                program: program.clone(), off: at.saturating_add(dur), voice: Some(voice) } });
        }
    }
}

fn preview_action(action: &Compiled, at: u64, sample_rate: f32, history: &mut Vec<HistoryEntry>, explicit: &mut Vec<(u64, u64, Event, u64)>) {
    let sample = |sec: f64| (sec * f64::from(sample_rate)).round().max(0.0) as u64;
    for event in &action.program.notes {
        let start = sample(event.at.to_seconds(action.program.tempo));
        let start = if action.statement.has_at { start.max(at) } else { at + start };
        explicit.push((start, sample(event.dur.to_seconds(action.program.tempo)), event.clone(), at));
    }
    for scheduled in &action.program.performance {
        let when = if action.statement.has_at { scheduled.at.to_seconds(action.program.tempo) } else { at as f64 / f64::from(sample_rate) };
        history.push(HistoryEntry { at: when, action: name_action(&action.program, &scheduled.action) });
    }
}

pub(super) fn schedule_for(program: &Program, history: &[HistoryEntry]) -> Schedule {
    let names = program.tracks.iter().map(|t| t.name.clone()).collect();
    let index = |name: &String| program.track_index(name);
    let commands = history.iter().filter_map(|h| {
        let action = match &h.action {
            NamedAction::Play { track, clip } => Action::Play { track: index(track)?, clip: clip.clone() },
            NamedAction::Stop(t) => Action::Stop { track: index(t)? },
            NamedAction::Mute(t) => Action::Mute { track: index(t)? },
            NamedAction::Unmute(t) => Action::Unmute { track: index(t)? },
            NamedAction::Solo(t) => Action::Solo { track: index(t)? },
            NamedAction::Unsolo(t) => Action::Unsolo { track: t.as_ref().and_then(index) },
            NamedAction::Hush => Action::Hush,
            NamedAction::Panic => Action::Panic,
        };
        Some(Scheduled { at: Time::Seconds(h.at), action, quantize: None })
    }).collect::<Vec<_>>();
    Schedule::from_commands(program.tempo, names, &commands)
}

impl RealtimeRenderer {
    pub fn clock(&self) -> u64 { self.clock }
    pub fn metrics(&self) -> RenderMetrics { self.state.metrics() }
    pub fn channels(&self) -> usize { self.channels }

    /// Invalidate buffered audio and request an off-thread clean mixer after
    /// the caller catches a DSP failure.
    pub fn request_panic(&self) {
        self.panic_epoch.fetch_add(1, Ordering::AcqRel);
        self.state.revision.fetch_add(1, Ordering::AcqRel);
    }

    fn retire_current(&mut self) -> bool {
        if self.current.is_none() { return true; }
        if self.retired.slots() == 0 { return false; }
        let Some(window) = self.current.take() else { return true };
        match self.retired.push(Retired::Window(window)) {
            Ok(()) => { self.cursor = 0; true }
            Err(PushError::Full(Retired::Window(window))) => { self.current = Some(window); false }
            Err(_) => unreachable!(),
        }
    }

    fn return_voices(&mut self) {
        while !self.finished.is_empty() && self.retired.slots() > 0 {
            let Some(voice) = self.finished.pop() else { break };
            match self.retired.push(Retired::Voice(voice)) {
                Ok(()) => {}
                Err(PushError::Full(Retired::Voice(voice))) => { self.finished.push(voice); break; }
                Err(_) => unreachable!(),
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
            let panic = self.panic_epoch.load(Ordering::Acquire);
            if panic != self.seen_panic {
                self.seen_panic = panic;
                self.mixer.silence();
                if let Some(limiter) = &mut self.limiter { limiter.clear(); }
                self.held.clear();
            }
            let revision = self.state.revision.load(Ordering::Acquire);
            loop {
                let done = self.current.as_ref().is_some_and(|w| w.revision != revision || (self.cursor == w.commands.len() && w.end <= self.clock));
                if done {
                    if stale_budget == 0 || !self.retire_current() { break; }
                    stale_budget -= 1;
                }
                if self.current.is_none() {
                    self.current = self.ready.pop().ok();
                    self.cursor = 0;
                    if self.current.as_ref().is_some_and(|w| w.revision != revision) { continue; }
                }
                break;
            }
            let ready = self.current.as_ref().is_some_and(|w| w.revision == revision
                && (w.end > self.clock || self.cursor < w.commands.len()));
            if ready {
                while let Some(command) = self.current.as_mut().and_then(|w| w.commands.get_mut(self.cursor)) {
                    if command.at > self.clock { break; }
                    if command.at < self.clock { self.state.late_events.fetch_add(1, Ordering::Relaxed); }
                    match &mut command.kind {
                        Kind::Note { track, program, off, voice } => {
                            let target = track.as_ref().map_or(Some(0), |name| self.program.track_index(name));
                            if let Some(target) = target
                                && *off > self.clock && self.held.len() < self.held.capacity()
                                && self.mixer.active_voices() < self.config.max_voices * 2
                                && let Some(mut prepared) = voice.take()
                            {
                                prepared.remap_buses_with(&mut |i| {
                                    program.buses.get(i).and_then(|name| self.program.buses.iter().position(|n| n == name))
                                        .filter(|&j| program.bus_layouts[i] == self.program.bus_layouts[j])
                                });
                                match self.mixer.note_prepared(target, prepared) {
                                    Ok(id) => self.held.push((*off, id)),
                                    Err(rejected) => { *voice = Some(rejected); self.state.capacity_drops.fetch_add(1, Ordering::Relaxed); }
                                }
                            } else { self.state.capacity_drops.fetch_add(1, Ordering::Relaxed); }
                        }
                        Kind::Swap { mixer, program, transfers } => {
                            transfers.clear();
                            for (i, track) in program.tracks.iter().enumerate() {
                                let old = if i == 0 { Some(0) } else { self.program.track_index(&track.name) };
                                transfers.push(Transfer { voices_from: old, chain_from: old.filter(|_| i > 0) });
                            }
                            self.mixer.remap_buses_with(&mut |i| {
                                self.program.buses.get(i).and_then(|name| program.buses.iter().position(|n| n == name))
                                    .filter(|&j| self.program.bus_layouts[i] == program.bus_layouts[j])
                            });
                            mixer.adopt(&mut self.mixer, transfers);
                            std::mem::swap(&mut self.mixer, mixer);
                            std::mem::swap(&mut self.program, program);
                        }
                        Kind::Reset { mixer, program, epoch } => {
                            std::mem::swap(&mut self.mixer, mixer);
                            std::mem::swap(&mut self.program, program);
                            if let Some(limiter) = &mut self.limiter { limiter.clear(); }
                            self.held.clear();
                            self.state.reset_epoch.store(*epoch, Ordering::Release);
                        }
                    }
                    self.cursor += 1;
                }
            }
            self.held.retain(|&(off, id)| { if off <= self.clock { self.mixer.release(id); false } else { true } });
            let mut n = (end - self.clock).min(BLOCK as u64);
            if ready && let Some(window) = &self.current {
                if let Some(command) = window.commands.get(self.cursor) { n = n.min(command.at.saturating_sub(self.clock).max(1)); }
                if window.end > self.clock { n = n.min(window.end - self.clock); }
            }
            for &(off, _) in &self.held { n = n.min(off.saturating_sub(self.clock).max(1)); }
            let n = n.max(1) as usize;
            if !ready { self.state.starved_frames.fetch_add(n, Ordering::Relaxed); }
            self.mixer.process_retained(&mut block[..self.channels], n, &mut self.finished);
            self.state.silenced.fetch_add(self.mixer.take_silenced(), Ordering::Relaxed);
            if let Some(limiter) = &mut self.limiter { limiter.process_block(&mut block[..self.channels], n); }
            for (peak, channel) in self.state.peaks.iter().zip(&block[..self.channels]) {
                let value = channel[..n].iter().fold(0.0f32, |m, s| m.max(s.abs()));
                peak.fetch_max(value.to_bits(), Ordering::Relaxed);
            }
            sink(&block[..self.channels], n);
            self.clock += n as u64;
            self.state.clock.store(self.clock, Ordering::Release);
            self.state.voices.store(self.mixer.active_voices(), Ordering::Relaxed);
        }
        self.return_voices();
        if self.retired.slots() == 0 { self.state.retire_pressure.fetch_add(1, Ordering::Relaxed); }
        let elapsed = started.elapsed();
        if frames > 0 {
            self.state.max_render_micros.fetch_max(elapsed.as_micros() as u64, Ordering::Relaxed);
            if elapsed.as_secs_f64() > frames as f64 / f64::from(self.sample_rate) {
                self.state.deadline_misses.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
