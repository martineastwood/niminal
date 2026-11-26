use super::*;

impl RealtimeRenderer {
    /// Send the notes of `midi_out` instruments down `tx`, stamped with the sample they belong to.
    pub fn set_midi_output(
        &mut self,
        tx: Producer<crate::MidiEvent>,
        wake: Option<std::thread::Thread>,
    ) {
        self.midi_tx = Some(tx);
        self.midi_wake = wake;
    }

    /// Every sounding `midi_out` note off, as `panic` and a reset do.
    fn all_midi_off(&mut self) {
        for (off, _, _, _) in &mut self.sounding.midi {
            *off = self.clock;
        }
        self.release_midi_due();
    }

    /// Retain overdue notes until their note-offs are queued, including after panic/reset.
    fn release_midi_due(&mut self) {
        let mut i = 0;
        while i < self.sounding.midi.len() {
            if self.sounding.midi[i].0 <= self.clock
                && send_midi(
                    &mut self.midi_tx,
                    &self.midi_wake,
                    self.clock,
                    false,
                    &self.sounding.midi[i].2,
                )
            {
                self.sounding.midi.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Feed nodes that read `input(..)` from this device audio from now on.
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
            let Ok(injected) = self.inject_rx.pop() else {
                break;
            };
            let order = injected.command.order();
            let place = self
                .injected
                .partition_point(|i| i.command.order() <= order);
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
            let Some(first) = self.injected.first() else {
                break;
            };
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

    /// Put a prepared mixer, program and controls in place, and record the sample at which the
    /// change was heard so that its client's receipt can say when it landed.
    fn install(&mut self, change: &mut GraphChange) {
        crate::controls::adopt(
            &mut change.controls,
            &self.controls,
            &change.program,
            self.clock,
            self.sample_rate,
        );
        for key in &change.stop_players {
            if let Some(control) = change.controls.iter_mut().find(|c| c.key == *key) {
                control.set(self.clock, 0., 0);
            }
        }
        crate::controls::sync_mixer(&change.controls, &mut change.mixer, self.clock);
        std::mem::swap(&mut self.controls, &mut change.controls);
        std::mem::swap(&mut self.mixer, &mut change.mixer);
        std::mem::swap(&mut self.program, &mut change.program);
        for restore in &mut change.mappings {
            if let Some(index) = self.program.node_index(&restore.node) {
                self.mixer
                    .swap_chain_mapping(index, &restore.param, &mut restore.mapping);
            }
        }
        for restore in &mut change.buffers {
            if let Some(index) = self.program.node_index(&restore.node) {
                self.mixer
                    .swap_chain_buffers(index, restore.key, &mut restore.banks);
            }
        }
        crate::controls::sync_mixer(&self.controls, &mut self.mixer, self.clock);
        if let Some(applied) = change.applied.take() {
            let _ = applied.compare_exchange(
                super::transactions::WAITING,
                self.clock,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }

    /// Carry out one prepared command at the current sample.
    pub(super) fn apply(&mut self, kind: &mut Kind) {
        self.mixer
            .set_clock(niminal_engine::Clock { sample: self.clock });
        match kind {
            Kind::ClientPanic {
                client,
                change,
                transfers,
            } => {
                change
                    .stop_players
                    .retain(|key| super::definitions::owns_node(*client, key));
                for (off, _, _, owner) in &mut self.sounding.midi {
                    if *owner == *client {
                        *off = self.clock;
                    }
                }
                self.release_midi_due();
                super::plan_transfers(&self.program, &change.program, transfers);
                self.mixer.silence_owner(*client);
                for (render_node, transfer) in change.program.nodes.iter().zip(transfers.iter_mut())
                {
                    if !render_node.name.is_empty()
                        && super::definitions::owns_node(*client, &render_node.name)
                    {
                        transfer.chain_from = None;
                        transfer.voices_from = None;
                    }
                }
                change.mixer.adopt(&mut self.mixer, transfers);
                self.install(change);
            }
            Kind::ChainMap {
                node: render_node,
                param,
                mapping,
            } => {
                if let Some(index) = self.program.node_index(render_node) {
                    if mapping.is_none()
                        && let Some(value) = self.mixer.chain_param_value(index, param)
                        && let Some(control) = self.controls.iter_mut().find(|c| c.key == *param)
                    {
                        control.set(self.clock, value, 0);
                    }
                    self.mixer.swap_chain_mapping(index, param, mapping);
                    crate::controls::sync_mixer(&self.controls, &mut self.mixer, self.clock);
                }
            }
            Kind::BindChain {
                node,
                resource,
                banks,
            } => {
                if let Some(index) = self.program.node_index(node) {
                    self.mixer.swap_chain_buffers(index, *resource, banks);
                }
            }
            Kind::BindVoice {
                key,
                resource,
                banks,
            } => {
                if let Some((_, id, _)) = self.sounding.held.iter().find(|h| h.2 == *key) {
                    self.mixer.swap_voice_buffers(*id, *resource, banks);
                }
            }
            Kind::Buffer {
                buffer,
                offset,
                values,
            } => {
                for (i, value) in values.iter().enumerate() {
                    buffer.write_sample(*offset * buffer.channels() + i, *value);
                }
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
            Kind::Map {
                key,
                param,
                mapping,
            } => {
                if let Some((_, id, _)) = self.sounding.held.iter().find(|h| h.2 == *key) {
                    self.mixer.swap_voice_mapping(*id, param, mapping);
                    crate::controls::sync_note(&self.controls, &mut self.mixer, *id, self.clock);
                }
            }
            Kind::SetParam {
                curve,
                key,
                name,
                value,
                frames,
            } => {
                for held in self.sounding.held.iter().filter(|h| h.2 == *key) {
                    self.mixer
                        .set_voice_param_glide(held.1, name, *value, *frames, *curve);
                }
            }
            Kind::Note {
                node: render_node,
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
                    if send_midi(&mut self.midi_tx, &self.midi_wake, self.clock, true, &note) {
                        self.sounding.midi.push((
                            *off,
                            *key,
                            note,
                            voice.as_ref().map_or(0, |v| v.owner()),
                        ));
                    } else {
                        self.state.capacity_drops.fetch_add(1, Ordering::Relaxed);
                    }
                }
                let target = render_node
                    .as_ref()
                    .map_or(Some(0), |name| self.program.node_index(name));
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
                                rejected.drop_activities(self.clock);
                                *voice = Some(rejected);
                                self.state.capacity_drops.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                } else {
                    if let Some(voice) = voice {
                        voice.drop_activities(self.clock);
                    }
                    self.state.capacity_drops.fetch_add(1, Ordering::Relaxed);
                }
            }
            Kind::Swap { change, transfers } => {
                super::plan_transfers(&self.program, &change.program, transfers);
                self.mixer.remap_buses_with(&mut |i| {
                    self.program
                        .buses
                        .get(i)
                        .and_then(|name| change.program.buses.iter().position(|n| n == name))
                        .filter(|&j| self.program.bus_layouts[i] == change.program.bus_layouts[j])
                });
                change.mixer.adopt(&mut self.mixer, transfers);
                self.mixer.finish_activities(self.clock, 4);
                self.install(change);
            }
            Kind::Reset { change, epoch } => {
                self.mixer.finish_activities(self.clock, 2);
                if let Some(limiter) = &mut self.limiter {
                    limiter.clear();
                }
                self.sounding.held.clear();
                self.all_midi_off();
                self.state.reset_epoch.store(*epoch, Ordering::Release);
                self.install(change);
            }
        }
    }

    /// This entire path is bounded, lock-free, allocation-free and free of
    /// heap destruction, including voice completion, edits and panic.
    pub fn render(&mut self, frames: usize, sink: &mut dyn FnMut(&[Vec<f32>], usize)) {
        let started = niminal_engine::time::Instant::now();
        let _flush = niminal_engine::denormals::FlushToZero::new();
        let end = self.clock.saturating_add(frames as u64);
        let max_block = niminal_engine::block_size() as u64;
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
                match change {
                    ControlMessage::Set(change) => self.control_pending.push(change),
                    ControlMessage::Retract { serial } => {
                        self.control_pending.retain(|c| c.serial != serial);
                    }
                }
            }
            if self.control_pending.len() > waiting {
                self.control_pending
                    .sort_unstable_by_key(|c| (c.at, c.serial));
            }
            self.mixer
                .set_clock(niminal_engine::Clock { sample: self.clock });
            let panic = self.panic_epoch.load(Ordering::Acquire);
            if panic != self.seen_panic {
                self.seen_panic = panic;
                self.mixer.finish_activities(self.clock, 2);
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
                // a knob moved "now" reaches the audio thread a block or so after the sample it was given: that is
                // how live input arrives, not a change that missed its deadline
                if change.at < self.clock && !change.now {
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
            let mut n = (end - self.clock).min(max_block);
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
                input.read(n, &mut self.heard);
                for (c, samples) in self.heard.iter().enumerate().take(input.channels()) {
                    self.mixer.set_audio_input(c, &samples[..n]);
                }
            }
            self.mixer
                .set_clock(niminal_engine::Clock { sample: self.clock });
            self.mixer
                .process_retained(&mut self.block[..self.channels], n, &mut self.finished);
            self.state
                .silenced
                .fetch_add(self.mixer.take_silenced(), Ordering::Relaxed);
            if let Some(limiter) = &mut self.limiter {
                limiter.process_block(&mut self.block[..self.channels], n);
            }
            let peaks = self
                .master_meter
                .observe(&self.block[..self.channels], n, self.clock);
            for (peak, value) in self.state.peaks.iter().zip(&peaks) {
                peak.fetch_max(value.to_bits(), Ordering::Relaxed);
            }
            sink(&self.block[..self.channels], n);
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

