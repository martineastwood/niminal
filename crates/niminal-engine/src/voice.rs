use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::graph::Graph;
use crate::mixer::{Buses, MAX_CHANNELS, Sends};
use crate::op_dispatch::Op;
use crate::opcode::{BLOCK, Opcode, ProcessCtx, TAIL_LEVEL};

/// One running note: a private copy of every opcode's state plus the signal
/// buffers that connect them.
pub struct Voice {
    /// Which chunk of its node the mixer sums this voice in.
    chunk: u64,
    owner: u64,
    instance: u64,
    realtime: bool,
    activities: Vec<Arc<crate::Activity>>,
    sample: u64,
    graph: Arc<Graph>,
    ops: Vec<Op>,
    slots: Vec<[f32; BLOCK]>,
    sample_rate: f32,
    released: bool,
    poisoned: bool,
    /// The choke group this note belongs to, if any.
    group: Option<u64>,
    bus_map: Vec<Option<usize>>,
    /// Samples of fade-out left once the voice has been choked.
    choke_left: Option<usize>,
    /// How many notes of this instrument may sound at once, and what a new note does to the others.
    rule: Option<VoiceRule>,
    /// Where the pitch parameter is on its way to, when the instrument glides or plays legato.
    glide: Glide,
    /// The notes held on a legato voice, which sounds the one its priority picks. Made along with the
    /// rule, so that starting a note never allocates.
    held: Option<Box<Held>>,
    /// The loudest sample of the last block, or the largest value before the voice has sounded.
    level: f32,
    /// Gains that place the voice's mix among the master's speakers, if this note has its own pan.
    pan: Option<Box<[f32; MAX_CHANNELS]>>,
    /// Whether an envelope or a sample decides when the voice ends. Without one, it ends at its
    /// release, after the graph's tail.
    shaped: bool,
    /// Samples of tail left after the release, for a voice that isn't shaped.
    tail_left: usize,
    /// Cached from the last render; avoids iterating all ops on `is_finished` / `is_quiet`.
    active_ops: bool,
    /// The scheduled length in seconds, 0 for a note a key holds open.
    dur: f32,
    /// How many notes of this instrument started before this one.
    index: u32,
    /// Parameters on their way to a new value. Made with room for every parameter, so that setting
    /// one never allocates.
    ramps: Vec<Ramp>,
    mappings: Vec<Option<Box<crate::ParamMapping>>>,
    last_frames: usize,
}

/// A parameter moving in a straight line to `to` over `frames`, `done` of which have passed.
struct Ramp {
    index: usize,
    from: f32,
    to: f32,
    frames: u64,
    done: u64,
}

impl Ramp {
    fn at(&self, done: u64) -> f32 {
        if done >= self.frames {
            self.to
        } else {
            (f64::from(self.from)
                + (f64::from(self.to) - f64::from(self.from)) * (done as f64 / self.frames as f64))
                as f32
        }
    }
}

/// Which held note a legato voice sounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Priority {
    #[default]
    Last,
    Lowest,
    Highest,
}

/// A limit on the notes one instrument has sounding on a node, and how a mono instrument joins them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceRule {
    /// Which instrument the notes are of, so the notes can be counted.
    pub instrument: u64,
    /// A new note ends one of them if this many already sound.
    pub max: usize,
    /// The one it ends is the quietest, not the oldest.
    pub steal_quietest: bool,
    /// A new note releases a sounding note of the same pitch.
    pub release_same_pitch: bool,
    /// The pitch of this note, in hz.
    pub pitch: f32,
    /// Which parameter is the pitch, if the instrument has one.
    pub pitch_param: Option<usize>,
    /// A note that starts while another is held changes the held voice's pitch instead of starting a new voice.
    pub legato: bool,
    pub priority: Priority,
    /// Seconds a change of pitch takes; 0 jumps.
    pub glide: f32,
}

/// A pitch moving to its target by a constant ratio per sample.
#[derive(Clone, Copy, Debug, Default)]
struct Glide {
    now: f32,
    target: f32,
    ratio: f32,
    left: usize,
}

impl Glide {
    fn start(&mut self, to: f32, seconds: f32, sample_rate: f32) {
        self.target = to;
        let frames = (seconds * sample_rate) as usize;
        if frames == 0 || self.now <= 0.0 || to <= 0.0 {
            self.now = to;
            self.left = 0;
        } else {
            self.ratio = (to / self.now).powf(1.0 / frames as f32);
            self.left = frames;
        }
    }

    fn step(&mut self) -> f32 {
        let value = self.now;
        if self.left > 0 {
            self.left -= 1;
            self.now = if self.left == 0 {
                self.target
            } else {
                self.now * self.ratio
            };
        }
        value
    }
}

/// The most notes a legato voice remembers; a ninth drops the oldest.
const HELD_NOTES: usize = 8;

#[derive(Clone, Copy, Debug, Default)]
struct Held {
    /// `(note id, pitch)` in the order the notes started.
    notes: [(u64, f32); HELD_NOTES],
    len: usize,
}

impl Held {
    fn push(&mut self, serial: u64, pitch: f32) {
        if self.len == HELD_NOTES {
            self.notes.copy_within(1.., 0);
            self.len -= 1;
        }
        self.notes[self.len] = (serial, pitch);
        self.len += 1;
    }

    fn remove(&mut self, serial: u64) {
        if let Some(i) = self.notes[..self.len].iter().position(|n| n.0 == serial) {
            self.notes.copy_within(i + 1..self.len, i);
            self.len -= 1;
        }
    }

    fn pick(&self, priority: Priority) -> Option<f32> {
        let mut pitches = self.notes[..self.len].iter().map(|n| n.1);
        match priority {
            Priority::Last => pitches.next_back(),
            Priority::Lowest => pitches.reduce(f32::min),
            Priority::Highest => pitches.reduce(f32::max),
        }
    }
}

/// How long a choked voice takes to fade out, in seconds.
const CHOKE_FADE: f32 = 0.005;

impl Voice {
    pub fn new(graph: Arc<Graph>, sample_rate: f32) -> Self {
        let mut ops: Vec<Op> = graph
            .nodes
            .iter()
            .map(|n| Op::from_boxed(n.op.box_clone()))
            .collect();
        for op in &mut ops {
            op.prepare(sample_rate);
        }

        let mut slots = vec![[0.0; BLOCK]; graph.node_slot_base() + graph.results];
        for (i, p) in graph.params.iter().enumerate() {
            slots[i] = [p.default; BLOCK];
        }
        for (i, &c) in graph.consts.iter().enumerate() {
            slots[graph.params.len() + i] = [c; BLOCK];
        }

        let bus_map = (0..graph.sends.iter().map(|s| s.0 + 1).max().unwrap_or(0))
            .map(Some)
            .collect();
        let shaped = ops.iter().any(|op| op.is_envelope());
        let active_ops = ops.iter().any(|op| op.is_active());
        let tail_left = (graph.tail * sample_rate) as usize;
        let graph_params = graph.params.len();
        Voice {
            chunk: 0,
            owner: 0,
            instance: 0,
            realtime: false,
            activities: Vec::new(),
            sample: 0,
            mappings: (0..graph_params).map(|_| None).collect(),
            last_frames: 1,
            graph,
            ops,
            slots,
            sample_rate,
            released: false,
            poisoned: false,
            group: None,
            bus_map,
            choke_left: None,
            rule: None,
            glide: Glide::default(),
            held: None,
            level: f32::MAX,
            pan: None,
            shaped,
            active_ops,
            tail_left,
            dur: 0.0,
            index: 0,
            ramps: Vec::with_capacity(graph_params),
        }
    }

    /// Keep memory at matching call sites while retaining the new graph's
    /// settings and wiring. Repeated identical nodes match in occurrence order.
    pub fn carry_state(&mut self, old: &mut Voice) {
        if self.sample_rate != old.sample_rate || old.poisoned {
            return;
        }
        for (index, param) in self.graph.params.iter().enumerate() {
            if let Some(previous) = old.graph.param_index(&param.name)
                && self.mappings[index].is_none()
                && old.mappings[previous].is_some()
            {
                self.mappings[index] = old.mappings[previous].take();
                self.mappings[index]
                    .as_mut()
                    .unwrap()
                    .set_range(param.range);
                self.slots[index] = old.slots[previous];
            }
        }
        let old_keys = old.graph.state_keys();
        let keys = self.graph.state_keys();
        for (new_index, (op, key)) in self.ops.iter_mut().zip(keys).enumerate() {
            let occurrence = keys[..new_index].iter().filter(|k| *k == key).count();
            if let Some(i) = old_keys
                .iter()
                .enumerate()
                .filter(|(_, k)| *k == key)
                .nth(occurrence)
                .map(|(i, _)| i)
            {
                op.carry_state(old.ops[i].inner_mut());
            }
        }
    }

    /// Update existing send mappings in place; the number of send identities
    /// belongs to the voice's original graph and never grows during playback.
    pub(crate) fn set_chunk(&mut self, chunk: u64) {
        self.chunk = chunk;
    }

    pub(crate) fn chunk(&self) -> u64 {
        self.chunk
    }

    pub fn remap_buses(&mut self, mapping: &[Option<usize>]) {
        self.remap_buses_with(&mut |i| mapping.get(i).copied().flatten());
    }

    pub fn remap_buses_with(&mut self, mapping: &mut impl FnMut(usize) -> Option<usize>) {
        for bus in &mut self.bus_map {
            *bus = bus.and_then(&mut *mapping);
        }
        for (index, map) in self.mappings.iter_mut().enumerate() {
            if let Some(map) = map
                && map.remap(mapping)
            {
                let value = self.slots[index][self.last_frames - 1];
                self.slots[index].fill(value);
            }
        }
    }

    /// Fill external input `index` for the coming block. Only the first
    /// `data.len()` samples are meaningful, so process no more than that.
    pub fn set_input(&mut self, index: usize, data: &[f32]) {
        let slot = self.graph.input_slot_base() + index;
        self.slots[slot][..data.len()].copy_from_slice(data);
    }

    pub fn set_control(&mut self, key: &str, start: f32, target: f32, frames: u64, elapsed: u64) {
        if let Some(index) = self.graph.param_index(key) {
            self.ramps.retain(|r| r.index != index);
            let ramp = Ramp {
                index,
                from: start,
                to: target,
                frames,
                done: elapsed.min(frames),
            };
            self.slots[index].fill(ramp.at(ramp.done));
            if elapsed < frames {
                self.ramps.push(ramp);
            }
        }
        for op in &mut self.ops {
            op.set_control(key, start, target, frames, elapsed);
        }
        for map in self.mappings.iter_mut().flatten() {
            map.set_control(key, start, target, frames, elapsed);
        }
    }

    pub fn param_value(&self, name: &str) -> Option<f32> {
        self.graph
            .param_index(name)
            .map(|i| self.slots[i][self.last_frames - 1])
    }
    pub fn param_index(&self, name: &str) -> Option<usize> {
        self.graph.param_index(name)
    }

    pub fn swap_buffers(&mut self, key: u64, banks: &mut [crate::BufferBank]) {
        let mut cursor = 0;
        for op in &mut self.ops {
            op.swap_buffers(key, banks, &mut cursor);
        }
    }
    pub fn swap_mapping(&mut self, name: &str, mapping: &mut Option<Box<crate::ParamMapping>>) {
        if let Some(index) = self.graph.param_index(name) {
            if mapping.is_none() {
                let value = self.slots[index][self.last_frames - 1];
                self.slots[index].fill(value);
            }
            self.ramps.retain(|r| r.index != index);
            if let Some(mapping) = mapping.as_mut() {
                mapping.set_range(self.graph.params[index].range);
            }
            std::mem::swap(&mut self.mappings[index], mapping);
        }
    }

    /// Set a parameter by index. Takes effect from the next block.
    pub fn set_param(&mut self, index: usize, value: f32) {
        self.slots[index] = [value; BLOCK];
    }

    /// Move a parameter to `value` in a straight line over `frames`. A glide is carried out as the voice is
    /// processed, so it starts from the first sample after this. Allocates nothing.
    pub fn set_param_glide(&mut self, index: usize, value: f32, frames: u64) {
        if index >= self.graph.params.len() {
            return;
        }
        let from = self.current_param(index);
        self.ramps.retain(|r| r.index != index);
        if frames == 0 {
            self.set_param(index, value);
        } else {
            self.ramps.push(Ramp {
                index,
                from,
                to: value,
                frames,
                done: 0,
            });
        }
    }

    /// Where a parameter is now, partway along a glide if it is on one.
    fn current_param(&self, index: usize) -> f32 {
        match self.ramps.iter().find(|r| r.index == index) {
            Some(r) => r.at(r.done),
            None => self.slots[index][0],
        }
    }

    /// Set a parameter by name; returns false if there is no such parameter.
    pub fn set_param_by_name(&mut self, name: &str, value: f32) -> bool {
        match self.graph.param_index(name) {
            Some(i) => {
                self.set_param(i, value);
                true
            }
            None => false,
        }
    }

    /// How long the note is scheduled to last, in seconds; 0 for a note a key holds open.
    pub fn set_dur(&mut self, seconds: f32) {
        self.dur = seconds;
    }

    /// Number this note among its instrument's. Called once, when the note actually starts.
    pub fn start_note(&mut self) {
        self.index = self.graph.notes_started.fetch_add(1, Ordering::Relaxed);
    }

    /// Note off. Envelopes with a sustain point begin their release.
    pub fn release(&mut self) {
        for a in &self.activities {
            a.release(self.sample);
        }
        self.released = true;
    }

    /// Put the note in a choke group: a later note in the same group ends it.
    pub fn set_group(&mut self, group: Option<u64>) {
        self.group = group;
    }

    pub fn group(&self) -> Option<u64> {
        self.group
    }

    /// Place this note among the speakers: its channels are mixed down and
    /// spread with `gains` (one per output channel; see `pan_gains`).
    pub fn set_pan(&mut self, gains: Option<[f32; MAX_CHANNELS]>) {
        self.pan = gains.map(Box::new);
    }

    pub fn set_rule(&mut self, rule: Option<VoiceRule>) {
        self.rule = rule;
        if let Some(rule) = rule {
            self.glide = Glide {
                now: rule.pitch,
                target: rule.pitch,
                ratio: 1.0,
                left: 0,
            };
            self.held = rule.legato.then(Box::<Held>::default);
        }
    }

    /// How loud the voice was in its last block.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// The pitch the voice is sounding right now, in hz.
    pub fn pitch_now(&self) -> f32 {
        self.glide.now
    }

    /// Start at `from` and glide to this note's own pitch.
    pub fn glide_from(&mut self, from: f32, sample_rate: f32) {
        let Some(rule) = self.rule else { return };
        let to = self.glide.target;
        self.glide.now = from;
        self.glide.start(to, rule.glide, sample_rate);
    }

    /// Make this the first note of a legato voice, which `serial` stands for.
    pub fn hold(&mut self, serial: u64) {
        let pitch = self.glide.target;
        if let Some(held) = &mut self.held {
            held.len = 0;
            held.push(serial, pitch);
        }
    }

    /// Whether this voice is playing a note as `serial`, one it took over from a later note.
    pub fn holds(&self, serial: u64) -> bool {
        self.held
            .as_ref()
            .is_some_and(|h| h.notes[..h.len].iter().any(|n| n.0 == serial))
    }

    /// Whether a new legato note can take this voice over.
    pub fn is_legato_open(&self) -> bool {
        self.activities.len() < 64
            && self.held.is_some()
            && !self.released
            && self.choke_left.is_none()
            && !self.poisoned
    }

    /// A later note, as `serial`, takes over this legato voice: its parameters apply, the envelopes
    /// carry on, and the pitch moves to whichever held note the priority picks.
    pub fn legato_on(&mut self, serial: u64, note: &Voice) {
        for a in &note.activities {
            if self.activities.len() < self.activities.capacity() {
                a.start(self.sample, serial);
                self.activities.push(a.clone());
            }
        }
        let Some(rule) = self.rule else { return };
        for i in 0..self.graph.params.len().min(note.graph.params.len()) {
            if Some(i) != rule.pitch_param {
                self.slots[i] = note.slots[i];
            }
        }
        if let Some(held) = &mut self.held {
            held.push(serial, note.glide.target);
        }
        self.retarget();
    }

    /// Note `serial` ended. A legato voice goes back to a note still held, or lets go if there is none.
    pub fn note_off(&mut self, serial: u64) {
        for a in &self.activities {
            if a.serial.load(Ordering::Relaxed) == serial {
                a.release(self.sample);
            }
        }
        let Some(held) = &mut self.held else {
            self.release();
            return;
        };
        held.remove(serial);
        if held.len == 0 {
            self.release();
        } else {
            self.retarget();
        }
    }

    fn retarget(&mut self) {
        let Some(rule) = self.rule else { return };
        let picked = self.held.as_ref().and_then(|h| h.pick(rule.priority));
        if let Some(pitch) = picked.filter(|p| *p != self.glide.target) {
            self.glide.start(pitch, rule.glide, self.sample_rate);
        }
    }

    pub fn attach_activity(&mut self, activity: Arc<crate::Activity>) {
        if self.activities.capacity() == 0 {
            self.activities.reserve(64);
        }
        self.activities.push(activity);
    }
    pub fn drop_activities(&self, sample: u64) {
        for a in &self.activities {
            a.drop_at(sample);
        }
    }
    pub fn start_activities(&self, sample: u64, serial: u64) {
        for a in &self.activities {
            a.start(sample, serial);
        }
    }
    pub fn end_reason(&self) -> u32 {
        if self.poisoned {
            3
        } else {
            self.activities
                .first()
                .map_or(0, |a| a.reason.load(Ordering::Relaxed))
        }
    }
    pub fn finish_activities(&self, sample: u64, reason: u32) {
        for a in &self.activities {
            a.finish(sample, reason);
        }
    }
    pub fn set_activity_sample(&mut self, sample: u64) {
        self.sample = sample;
    }
    pub fn set_realtime(&mut self) {
        self.realtime = true;
    }
    pub fn set_instance(&mut self, instance: u64) {
        self.instance = instance;
    }
    pub fn set_owner(&mut self, owner: u64) {
        self.owner = owner;
    }
    pub fn owner(&self) -> u64 {
        self.owner
    }
    pub fn silence(&mut self) {
        self.finish_activities(self.sample, 2);
        self.released = true;
        self.choke_left = Some(0);
    }

    pub fn rule(&self) -> Option<VoiceRule> {
        self.rule
    }

    /// End the note now, with a short fade so it doesn't click.
    pub fn choke(&mut self) {
        for a in &self.activities {
            a.reason.store(1, Ordering::Relaxed);
            a.release(self.sample);
        }
        self.released = true;
        if self.choke_left.is_none() {
            self.choke_left = Some(((self.sample_rate * CHOKE_FADE) as usize).max(1));
        }
    }

    pub fn is_choked(&self) -> bool {
        self.choke_left.is_some()
    }

    pub fn is_released(&self) -> bool {
        self.released
    }

    /// A voice lives until it has been released and every envelope has
    /// finished (and anything they fed, such as a delay, has rung out), which may
    /// be well after the scheduled note length. A tail keeps it sounding that long
    /// after its release, then it fades out. A voice with no envelope fades out at
    /// its release, or after its tail.
    pub fn is_finished(&self) -> bool {
        self.poisoned
            || self.choke_left == Some(0)
            || (self.released
                && self.shaped
                && self.graph.tail == 0.0
                && self.choke_left.is_none()
                && !self.active_ops)
    }

    /// Whether the last `frames` samples put nothing audible out (to the outputs or the buses) and no
    /// opcode has a tail left to ring out.
    pub fn is_quiet(&self, frames: usize) -> bool {
        let quiet = |slot: usize| {
            self.slots[slot][..frames]
                .iter()
                .all(|s| s.abs() < TAIL_LEVEL)
        };
        self.level < TAIL_LEVEL
            && !self.active_ops
            && self.graph.sends.iter().all(|&(_, _, slot, _)| quiet(slot))
    }

    /// True once the voice produced NaN or infinity. It is silenced from then
    /// on (its state can't be trusted) and counts as finished.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Number of output channels.
    pub fn channels(&self) -> usize {
        self.graph.channels()
    }

    /// Render `out.len()` samples (at most [`BLOCK`]) of a one-channel graph.
    /// Output is identical however a stretch of audio is divided into calls, so
    /// a host can split blocks at event times for sample-accurate timing.
    ///
    /// Anything the graph sends to a bus is dropped; use [`Voice::process_routed`]
    /// for graphs that send.
    pub fn process(&mut self, out: &mut [f32]) {
        self.process_routed(out, &mut Buses::new(&[]));
    }

    /// Like [`Voice::process`], adding the graph's sends into `buses`.
    pub fn process_routed(&mut self, out: &mut [f32], buses: &mut Buses) {
        assert_eq!(
            self.channels(),
            1,
            "this graph has {} channels; use process_blocks",
            self.channels()
        );
        let frames = out.len();
        self.sample = buses.clock.sample;
        assert!(frames <= BLOCK, "block too long: {frames} > {BLOCK}");
        let mut block = [[0.0f32; BLOCK]; 1];
        self.process_blocks(&mut block, frames, buses);
        out.copy_from_slice(&block[0][..frames]);
    }

    /// Render `frames` samples (at most [`BLOCK`]) of every output channel into
    /// `out`, which must have one block per channel, and add the graph's sends
    /// to `buses`.
    pub fn process_blocks(&mut self, out: &mut [[f32; BLOCK]], frames: usize, buses: &mut Buses) {
        if let Some(fade) = self.render(out, frames, buses) {
            self.send(
                frames,
                fade.as_ref(),
                &mut |bus, channel, delayed, samples| {
                    if delayed {
                        buses.add_delayed(bus, channel, samples);
                    } else {
                        buses.add(bus, channel, samples);
                    }
                },
            );
            self.settle_ramps();
        }
    }

    /// Like [`Voice::process_blocks`], but the graph's sends go into `sends`, to be added to the
    /// buses later, so voices that run side by side never write to the same place.
    pub(crate) fn process_sending(
        &mut self,
        out: &mut [[f32; BLOCK]],
        frames: usize,
        buses: &Buses,
        sends: &mut Sends,
    ) {
        if let Some(fade) = self.render(out, frames, buses) {
            self.send(
                frames,
                fade.as_ref(),
                &mut |bus, channel, delayed, samples| {
                    if delayed {
                        sends.add_delayed(bus, channel, samples);
                    } else {
                        sends.add(bus, channel, samples);
                    }
                },
            );
            self.settle_ramps();
        }
    }

    /// Everything but the sends. `None` when the voice has nothing to send; otherwise the choke fade
    /// the sends must be given, if there is one.
    fn render(
        &mut self,
        out: &mut [[f32; BLOCK]],
        frames: usize,
        buses: &Buses,
    ) -> Option<Option<[f32; BLOCK]>> {
        self.sample = buses.clock.sample;
        assert!(frames <= BLOCK, "block too long: {frames} > {BLOCK}");
        assert_eq!(out.len(), self.channels(), "one output block per channel");
        if self.poisoned || self.choke_left == Some(0) {
            out.iter_mut().for_each(|c| c[..frames].fill(0.0));
            return None;
        }
        // after its release a voice rings for its tail, then fades out: without an envelope once the
        // tail is over, with one once its envelopes are done as well
        if self.released && self.choke_left.is_none() {
            let over = self.tail_left == 0;
            self.tail_left = self.tail_left.saturating_sub(frames);
            let done = if self.shaped {
                over && self.graph.tail > 0.0 && !self.ops.iter().any(|op| op.has_active_envelope())
            } else {
                over
            };
            if done {
                self.choke();
            }
        }
        let ctx = ProcessCtx {
            realtime: self.realtime,
            note_token: self.activities.last().map_or(0, |a| a.token),
            instance: self.instance,
            sample_rate: self.sample_rate,
            gate: !self.released,
            dur: self.dur,
            index: self.index,
            clock: buses.clock,
        };

        for ramp in &mut self.ramps {
            for (n, sample) in self.slots[ramp.index][..frames].iter_mut().enumerate() {
                *sample = ramp.at(ramp.done + n as u64 + 1);
            }
            ramp.done += frames as u64;
        }

        if let Some(param) = self.rule.and_then(|r| r.pitch_param)
            && (self.glide.left > 0 || self.glide.now != self.slots[param][0])
        {
            for sample in &mut self.slots[param][..frames] {
                *sample = self.glide.step();
            }
        }

        for (index, mapping) in self.mappings.iter_mut().enumerate() {
            if let Some(mapping) = mapping {
                mapping.process(&ctx, buses, &mut self.slots[index][..frames]);
            }
        }
        self.last_frames = frames.max(1);
        let base = self.graph.node_slot_base();
        for (i, op) in self.ops.iter_mut().enumerate() {
            let node = &self.graph.nodes[i];
            let (before, after) = self.slots.split_at_mut(base + node.first);

            op.process_slots(
                &ctx,
                before,
                &node.inputs,
                &mut after[..node.outputs],
                frames,
            );
        }
        self.active_ops = self.ops.iter().any(|op| op.is_active());

        let finite = |slot: usize| self.slots[slot][..frames].iter().all(|s| s.is_finite());
        let healthy = self.graph.output_slots.iter().all(|&s| finite(s))
            && self.graph.sends.iter().all(|&(_, _, s, _)| finite(s));
        if !healthy {
            self.poisoned = true;
            out.iter_mut().for_each(|c| c[..frames].fill(0.0));
            return None;
        }
        // A choked voice fades linearly to nothing over `CHOKE_FADE`.
        let fade = self.choke_left.map(|left| {
            let total = ((self.sample_rate * CHOKE_FADE) as usize).max(1) as f32;
            let mut gains = [0.0f32; BLOCK];
            for (n, g) in gains[..frames].iter_mut().enumerate() {
                *g = (left.saturating_sub(n) as f32 / total).max(0.0);
            }
            self.choke_left = Some(left.saturating_sub(frames));
            gains
        });
        let mut level = 0.0f32;
        for (channel, &slot) in out.iter_mut().zip(&self.graph.output_slots) {
            channel[..frames].copy_from_slice(&self.slots[slot][..frames]);
            if let Some(g) = &fade {
                channel[..frames]
                    .iter_mut()
                    .zip(g)
                    .for_each(|(y, g)| *y *= g);
            }
        }
        if let Some(gains) = self.pan.as_deref()
            && out.len() > 1
        {
            // the mix of every channel, as loud as one channel of a mono voice, placed with the gains
            let share = 1.0 / (out.len() as f32).sqrt();
            for n in 0..frames {
                let mix = out.iter().map(|c| c[n]).sum::<f32>() * share;
                out.iter_mut().zip(gains).for_each(|(c, g)| c[n] = mix * g);
            }
        }
        for channel in out.iter() {
            level = channel[..frames].iter().fold(level, |l, y| l.max(y.abs()));
        }
        self.level = level;
        Some(fade)
    }

    fn send(
        &self,
        frames: usize,
        fade: Option<&[f32; BLOCK]>,
        add: &mut impl FnMut(usize, usize, bool, &[f32]),
    ) {
        for &(bus, channel, slot, delayed) in &self.graph.sends {
            let Some(bus) = self.bus_map.get(bus).copied().flatten() else {
                continue;
            };
            match fade {
                None => add(bus, channel, delayed, &self.slots[slot][..frames]),
                Some(g) => {
                    let mut faded = [0.0f32; BLOCK];
                    faded[..frames]
                        .iter_mut()
                        .zip(&self.slots[slot])
                        .zip(g)
                        .for_each(|((y, x), g)| *y = x * g);
                    add(bus, channel, delayed, &faded[..frames]);
                }
            }
        }
    }

    fn settle_ramps(&mut self) {
        for i in (0..self.ramps.len()).rev() {
            if self.ramps[i].done >= self.ramps[i].frames {
                let Ramp { index, to, .. } = self.ramps.swap_remove(i);
                self.slots[index] = [to; BLOCK];
            }
        }
    }
}
