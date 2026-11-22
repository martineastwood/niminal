use super::*;

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
        mixer.set_fork(self.fork.clone());
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
            master_meter: self.master_meter.clone(),
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
            block: niminal_engine::blocks(MAX_CHANNELS),
            heard: niminal_engine::blocks(MAX_CHANNELS),
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
    pub(in crate::session) fn invalidate_live(&self) {
        if let Some(planner) = &self.live {
            planner.state.swap_epoch.fetch_add(1, Ordering::AcqRel);
            planner.state.revision.fetch_add(1, Ordering::AcqRel);
        }
        self.wake_planner();
    }

    /// Something waiting was taken back: what is prepared is thrown away and prepared again, with the
    /// mixer as it is.
    pub(in crate::session) fn invalidate_windows(&self) {
        if let Some(planner) = &self.live {
            planner.state.revision.fetch_add(1, Ordering::AcqRel);
        }
        self.wake_planner();
    }

    /// Ask the planner to look at what was added, without throwing away what it has prepared.
    pub(in crate::session) fn wake_planner(&self) {
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
                && let Some(Op::Control { at, .. }) = ops.get_mut(applied.log_op)
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
        let unapplied_graph = self.pending.iter().any(|p| {
            p.lands_at < now && p.applied.load(Ordering::Acquire) == super::transactions::WAITING
        });
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
            if let Some(set) = self.node_sets.iter().find(|s| !s.done)
                && set.at < now
            {
                next = next.min(set.at.max(self.clock));
            }
            self.clock = next;
            self.land_due(now);
            self.apply_controls();
            self.apply_node_sets(if self.clock < now {
                self.clock.saturating_add(1)
            } else {
                self.clock
            });
        }
        self.land_due(now);
        self.apply_controls();
        self.apply_node_sets(self.clock);
        if unapplied_graph {
            self.invalidate_live();
        }
        for set in &mut self.buffer_sets {
            if set.at < self.clock {
                set.done = true;
            }
        }
        let stale = self.clock.saturating_sub(self.sample_rate as u64 * 2);
        self.buffer_sets.retain(|s| !s.done || s.at > stale);
        for bind in &mut self.buffer_binds {
            if bind.at < self.clock {
                bind.done = true;
            }
        }
        self.prune_buffer_binds(stale);
        for map in &mut self.chain_maps {
            if map.at < self.clock {
                map.done = true;
            }
        }
        self.prune_chain_maps();
        for set in &mut self.voice_maps {
            if set.at < self.clock {
                set.done = true;
            }
        }
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
        // Keep the planner owned across failures in retirement, injection and window construction.
        let planned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
            self.plan_ahead(&mut planner, revision, max_windows)
        }));
        let more = planned.as_ref().is_ok_and(|more| *more);
        if planned.is_err() {
            planner.state.planner_panics.fetch_add(1, Ordering::Relaxed);
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
                "{} notes exceeded live voice or MIDI output capacity",
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
        if planned.is_err() {
            self.restart_realtime();
        }
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
                Fresh::Note {
                    event,
                    start,
                    dur,
                    key,
                    activity,
                } => {
                    let hold = Hold {
                        at: start.max(self.clock),
                        dur,
                        key,
                        activity,
                    };
                    self.prepare_note(&program, &event, hold, config, &mut window);
                }
                Fresh::Release { key, at } => window.commands.push(Command {
                    at: at.max(self.clock),
                    kind: Kind::Release { key },
                }),
                Fresh::ChainMap(map) => window.commands.push(chain_map_command(
                    self.program_at(map.at),
                    &map,
                    map.at.max(self.clock),
                )),
                Fresh::Bind(set) => window
                    .commands
                    .push(bind_command(&set, set.at.max(self.clock))),
                Fresh::Map(set) => window.commands.push(map_command(
                    self.program_at(set.at),
                    &set,
                    set.at.max(self.clock),
                )),
                Fresh::Buffer(set) => {
                    if let Some((_, buffer)) = self
                        .program_at(set.at)
                        .buffers
                        .iter()
                        .find(|(name, _)| *name == set.name)
                        && set.offset <= buffer.len()
                        && set.values.len() <= (buffer.len() - set.offset) * buffer.channels()
                    {
                        window.commands.push(Command {
                            at: set.at.max(self.clock),
                            kind: Kind::Buffer {
                                buffer: buffer.clone(),
                                offset: set.offset,
                                values: set.values,
                            },
                        });
                    }
                }
                Fresh::Node(set) => window.commands.push(Command {
                    at: set.at.max(self.clock),
                    kind: Kind::Control {
                        key: set.key,
                        value: set.value,
                        frames: set.frames,
                    },
                }),
                Fresh::Set(set) => {
                    if let Some(command) =
                        set_command(&program, &set, set.at.max(self.clock), set.frames)
                    {
                        window.commands.push(command);
                    }
                }
            }
            let full = window.commands.len() > planner.inject_tx.slots();
            for notice in window.notices.drain(..) {
                self.notify(notice);
            }
            planner
                .state
                .capacity_drops
                .fetch_add(window.dropped, Ordering::Relaxed);
            if full {
                // the windows are made again, which the next step will see (the planner is out of the
                // session while it works, so this can't go through `invalidate_windows`)
                self.fresh.clear();
                planner.state.revision.fetch_add(1, Ordering::AcqRel);
                return;
            }
            for command in window.commands {
                let injected = Box::new(Injected {
                    revision: planner.revision,
                    command,
                });
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
                activity: n.activity.clone(),
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
                if let Kind::Swap { change, .. } = &mut swap.kind {
                    change.applied = Some(pending.applied.clone());
                    controls::mark(&mut change.controls, next, &pending.statements, pending.id);
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
        mixer.set_fork(self.fork.clone());
        Box::new(mixer)
    }

    fn swap_command(&self, at: u64, program: &Arc<Program>, config: RealtimeConfig) -> Command {
        let mixer = self.fresh_mixer(program, config);
        let states = self.control_states(program);
        Command {
            at,
            kind: Kind::Swap {
                change: GraphChange {
                    mappings: self.chain_restores(program, at),
                    buffers: self.chain_buffer_restores(at),
                    stop_players: Vec::new(),
                    applied: self.acknowledged(program),
                    mixer,
                    program: program.clone(),
                    controls: states,
                },
                transfers: Vec::with_capacity(program.nodes.len()),
            },
        }
    }

    /// The transaction a client is waiting on, when `program` is the one it asked for.
    fn acknowledged(&self, program: &Arc<Program>) -> Option<Arc<AtomicU64>> {
        if Arc::ptr_eq(program, &self.program) {
            self.current_transaction.clone()
        } else {
            None
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

    fn chain_buffer_restores(&self, at: u64) -> Vec<BufferRestore> {
        super::parameters::latest_chain_binds(&self.buffer_binds, at)
            .filter_map(|(id, bind)| {
                let (key, count, _) = self
                    .routing_at(at)
                    .buffer_binding(bind.client, id, &bind.param)
                    .ok()?;
                Some(BufferRestore {
                    node: crate::routing::fx_name(bind.client, id),
                    key,
                    banks: vec![bind.bank.clone(); count],
                })
            })
            .collect()
    }
    fn chain_restores(&self, program: &Program, at: u64) -> Vec<ChainRestore> {
        super::parameters::latest_chain_maps(&self.chain_maps, at)
            .filter(|m| {
                m.source.is_some() && program.controls.iter().any(|c| c.key == m.control_key)
            })
            .map(|m| ChainRestore {
                node: m.node(),
                param: m.control_key.clone(),
                mapping: super::parameters::prepare_mapping(
                    program,
                    m.source.as_deref(),
                    m.channel,
                    m.scale,
                    m.offset,
                    None,
                ),
            })
            .collect()
    }
    pub(super) fn reset_command(
        &self,
        at: u64,
        program: &Arc<Program>,
        config: RealtimeConfig,
    ) -> Command {
        let mixer = self.fresh_mixer(program, config);
        Command {
            at,
            kind: Kind::Reset {
                change: GraphChange {
                    mappings: self.chain_restores(program, at),
                    buffers: self.chain_buffer_restores(at),
                    stop_players: self.routing_at(at).player_keys(None),
                    applied: self.acknowledged(program),
                    mixer,
                    program: program.clone(),
                    controls: self.control_states(program),
                },
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
                let mut change = self.reset_command(panic.at, program, config);
                if let Kind::Reset { change: graph, .. } = change.kind {
                    change.kind = Kind::ClientPanic {
                        client: panic.client,
                        change: graph,
                        transfers: Vec::with_capacity(program.nodes.len()),
                    };
                }
                window.commands.push(change);
            }
        }
        for note in explicit {
            if note.start >= from && note.start < to {
                self.prepare_note(
                    program,
                    &note.event,
                    Hold {
                        at: note.start,
                        dur: note.dur,
                        key: note.key,
                        activity: note.activity.clone(),
                    },
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
        for map in &self.chain_maps {
            if map.at >= from && map.at < to {
                window
                    .commands
                    .push(chain_map_command(program, map, map.at));
            }
        }
        for set in &self.buffer_binds {
            if set.at >= from && set.at < to {
                window.commands.push(bind_command(set, set.at));
            }
        }
        for set in &self.voice_maps {
            if set.at >= from && set.at < to {
                window.commands.push(map_command(program, set, set.at));
            }
        }
        for set in &self.buffer_sets {
            if set.at >= from
                && set.at < to
                && let Some((_, buffer)) =
                    program.buffers.iter().find(|(name, _)| *name == set.name)
                && set.offset <= buffer.len()
                && set.values.len() <= (buffer.len() - set.offset) * buffer.channels()
            {
                window.commands.push(Command {
                    at: set.at,
                    kind: Kind::Buffer {
                        buffer: buffer.clone(),
                        offset: set.offset,
                        values: set.values.clone(),
                    },
                });
            }
        }
        // render_node parameters moved by `node` events
        let mut moved = 0;
        for set in &self.node_sets {
            if set.at >= from && set.at < to {
                moved += 1;
                if moved > MAX_WINDOW_CONTROLS {
                    window.notice("an envelope lane changes a parameter too often; the extra changes were dropped");
                    break;
                }
                window.commands.push(Command {
                    at: set.at,
                    kind: Kind::Control {
                        key: set.key.clone(),
                        value: set.value,
                        frames: set.frames,
                    },
                });
            }
        }
        // releases: each key is its own note, so a release that is repeated in a later window does no harm
        for off in &self.key_offs {
            if off.at < to {
                window.commands.push(Command {
                    at: off.at.max(from),
                    kind: Kind::Release { key: off.key },
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
        let Hold {
            at,
            dur,
            key,
            activity,
        } = when;
        if window.commands.len() >= config.max_voices * 4 {
            if let Some(a) = activity {
                a.drop_at(at);
            }
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
            if let Some(activity) = activity {
                voice.attach_activity(activity);
            }
            voice.set_realtime();
            voice.set_owner(super::definitions::owner(
                &program.instruments[plan.instrument].name,
            ));
            voice.set_group(plan.choke);
            voice.set_rule(plan.rule);
            voice.set_pan(plan.pan);
            voice.set_dur(if key.is_some() && dur == 0 {
                0.0
            } else {
                dur as f32 / self.sample_rate
            });
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
                    node: (plan.node != 0).then(|| program.nodes[plan.node].name.clone()),
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
