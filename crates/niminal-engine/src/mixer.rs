//! Tracks, buses and the master: everything between voices and the speakers.
//!
//! Each block, buses are cleared; then tracks run in dependency order, so
//! everything that writes a bus runs before anything that reads it. That is
//! why a send reaches its reader in the same block, with no added latency.
//! The order is worked out from the graphs themselves, so the order tracks are
//! declared in never matters.

use std::sync::{Arc, atomic::{AtomicU32, Ordering}};

use crate::graph::Graph;
use crate::opcode::BLOCK;
use crate::voice::{Voice, VoiceRule};

/// The most channels any signal path may carry (7.1.4 needs 12).
pub const MAX_CHANNELS: usize = 16;

/// Peaks shared with the control thread; rendering never allocates or locks.
#[derive(Debug, Default)]
pub struct Meter {
    peaks: [AtomicU32; MAX_CHANNELS],
    channels: AtomicU32,
}
impl Meter {
    fn observe(&self, blocks: &[[f32; BLOCK]], frames: usize) {
        self.channels.store(blocks.len() as u32, Ordering::Relaxed);
        for (peak, block) in self.peaks.iter().zip(blocks) {
            let value = block[..frames].iter().fold(0.0_f32, |a, b| a.max(b.abs()));
            peak.fetch_max(value.to_bits(), Ordering::Relaxed);
        }
    }
    pub fn read(&self, take: bool) -> Vec<f32> {
        self.peaks[..self.channels.load(Ordering::Relaxed) as usize]
            .iter()
            .map(|p| {
                f32::from_bits(if take {
                    p.swap(0, Ordering::Relaxed)
                } else {
                    p.load(Ordering::Relaxed)
                })
            })
            .collect()
    }
}

/// Block-sized, possibly multichannel buffers that are summed into and cleared
/// every block.
pub struct Buses {
    data: Vec<Vec<[f32; BLOCK]>>,
    /// What delayed sends put in over the last `BLOCK` samples, as a ring: a sample comes out
    /// where it went in, `BLOCK` samples later.
    late: Vec<Vec<[f32; BLOCK]>>,
    /// What delayed sends have put in during the current block.
    staged: Vec<Vec<[f32; BLOCK]>>,
    late_pos: usize,
    history: Vec<Vec<[f32; BLOCK]>>,
    /// Where the session's clock is, for the block being mixed.
    pub clock: crate::Clock,
}

impl Buses {
    /// One bus per entry, with that many channels.
    pub fn new(channels: &[usize]) -> Self {
        Buses {
            data: channels.iter().map(|&n| vec![[0.0; BLOCK]; n]).collect(),
            late: channels.iter().map(|&n| vec![[0.0; BLOCK]; n]).collect(),
            staged: channels.iter().map(|&n| vec![[0.0; BLOCK]; n]).collect(),
            late_pos: 0,
            history: channels.iter().map(|&n| vec![[0.0; BLOCK];n]).collect(),
            clock: crate::Clock::default(),
        }
    }

    /// Empty every bus, including what is on its way.
    pub fn clear(&mut self) {
        for bus in self.data.iter_mut().chain(&mut self.late).chain(&mut self.staged).chain(&mut self.history) {
            bus.iter_mut().for_each(|c| *c = [0.0; BLOCK]);
        }
        self.late_pos = 0;
    }

    /// Start a block of `frames` samples: the buses hold what delayed sends put in a block ago
    /// (exactly `BLOCK` samples ago, whatever the length of the blocks in between).
    pub fn begin_block(&mut self, frames: usize) {
        for ((data, late), staged) in self.data.iter_mut().zip(&mut self.late).zip(&mut self.staged) {
            for ((d, l), s) in data.iter_mut().zip(late.iter_mut()).zip(staged.iter_mut()) {
                *s = [0.0; BLOCK];
                *d = [0.0; BLOCK];
                for (i, out) in d[..frames].iter_mut().enumerate() {
                    let at = (self.late_pos + i) % BLOCK;
                    *out = l[at];
                    l[at] = 0.0;
                }
            }
        }
    }

    /// Finish the block: what delayed sends staged waits to arrive in `BLOCK` samples.
    pub fn end_block(&mut self, frames: usize) {
        for (history,data) in self.history.iter_mut().zip(&self.data) {
            for (h,d) in history.iter_mut().zip(data) { for i in 0..frames { h[(self.late_pos+i)%BLOCK] = d[i]; } }
        }
        for (late, staged) in self.late.iter_mut().zip(&self.staged) {
            for (l, s) in late.iter_mut().zip(staged) {
                for i in 0..frames {
                    l[(self.late_pos + i) % BLOCK] += s[i];
                }
            }
        }
        self.late_pos = (self.late_pos + frames) % BLOCK;
    }

    /// Add `samples` to the start of one channel of a bus. A bus or channel that
    /// doesn't exist is ignored.
    pub fn add(&mut self, bus: usize, channel: usize, samples: &[f32]) {
        if let Some(c) = self.data.get_mut(bus).and_then(|b| b.get_mut(channel)) {
            for (d, s) in c.iter_mut().zip(samples) {
                *d += s;
            }
        }
    }

    /// Like [`Buses::add`], but the samples arrive `BLOCK` samples later.
    pub fn add_delayed(&mut self, bus: usize, channel: usize, samples: &[f32]) {
        if let Some(c) = self.staged.get_mut(bus).and_then(|b| b.get_mut(channel)) {
            for (d, s) in c.iter_mut().zip(samples) {
                *d += s;
            }
        }
    }

    pub fn get(&self, bus: usize, channel: usize, frames: usize) -> &[f32] {
        &self.data[bus][channel][..frames]
    }

    /// Runtime bus mappings intentionally read exactly one block behind, avoiding dynamic cycles.
    pub fn previous(&self, bus: usize, channel: usize, i: usize) -> f32 { self.history.get(bus).and_then(|b| b.get(channel)).map_or(0.0,|c| c[(self.late_pos+i)%BLOCK]) }

    pub fn channels(&self, bus: usize) -> usize {
        self.data[bus].len()
    }
}

/// Where a track's output goes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    #[default]
    Master,
    Bus(usize),
}

/// What feeds one of a track chain's external (mono) inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainInput {
    /// One channel of the summed output of the track's voices.
    It { channel: usize },
    /// One channel of a bus.
    Bus { bus: usize, channel: usize },
    /// One channel of the audio input (a microphone, a line in, a file being rendered).
    Audio { channel: usize },
}

#[derive(Clone, Default)]
pub struct TrackDef {
    pub meter: Option<Arc<Meter>>,
    /// Replacement generation and crossfade duration in seconds.
    pub transition: (u64, f64),
    /// Public instance control keys mapped to keys in the reusable DSP graph.
    pub controls: Vec<(String, String)>,
    /// Processing applied to the summed voices. `None` passes them through.
    pub chain: Option<Arc<Graph>>,
    /// What each of the chain graph's inputs receives, in input order.
    pub inputs: Vec<ChainInput>,
    pub route: Route,
    /// Buses that this track's voices may send to.
    pub voice_sends: Vec<usize>,
    /// Shared buffers that this track's voices may write.
    pub voice_buffer_writes: Vec<usize>,
    /// Shared buffers that this track's voices may read.
    pub voice_buffer_reads: Vec<usize>,
}

impl TrackDef {
    fn reads(&self) -> Vec<usize> {
        self.inputs
            .iter()
            .filter_map(|i| match i {
                ChainInput::Bus { bus, .. } => Some(*bus),
                ChainInput::It { .. } | ChainInput::Audio { .. } => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn writes(&self) -> Vec<usize> {
        let mut w = self.voice_sends.clone();
        if let Some(chain) = &self.chain {
            w.extend(chain.send_buses());
        }
        if let Route::Bus(b) = self.route {
            w.push(b);
        }
        w
    }

    fn buffer_reads(&self) -> Vec<usize> {
        let mut r = self.voice_buffer_reads.clone();
        if let Some(chain) = &self.chain {
            r.extend(chain.buffer_reads().iter().copied());
        }
        r.sort_unstable();
        r.dedup();
        r
    }

    fn buffer_writes(&self) -> Vec<usize> {
        let mut w = self.voice_buffer_writes.clone();
        if let Some(chain) = &self.chain {
            w.extend(chain.buffer_writes().iter().copied());
        }
        w.sort_unstable();
        w.dedup();
        w
    }
}

/// An order in which to run tracks so that every bus and shared buffer is
/// written before it is read. A cycle comes back as the tracks that form it, in
/// order; a track that both writes and reads one bus is a cycle of one. A track
/// that both writes and reads the same buffer does not count as a cycle (the
/// write and read order inside its graph already decides that).
pub fn execution_order(tracks: &[TrackDef]) -> Result<Vec<usize>, Vec<usize>> {
    let n = tracks.len();
    let reads: Vec<_> = tracks.iter().map(TrackDef::reads).collect();
    let writes: Vec<_> = tracks.iter().map(TrackDef::writes).collect();
    let buf_reads: Vec<_> = tracks.iter().map(TrackDef::buffer_reads).collect();
    let buf_writes: Vec<_> = tracks.iter().map(TrackDef::buffer_writes).collect();
    // edges[w] lists tracks that must run after w
    let edges: Vec<Vec<usize>> = (0..n)
        .map(|w| {
            (0..n)
                .filter(|&r| {
                    writes[w].iter().any(|b| reads[r].contains(b))
                        || (w != r && buf_writes[w].iter().any(|b| buf_reads[r].contains(b)))
                })
                .collect()
        })
        .collect();

    let mut waiting_on = vec![0usize; n];
    for targets in &edges {
        for &r in targets {
            waiting_on[r] += 1;
        }
    }

    let mut order = Vec::with_capacity(n);
    let mut ready: Vec<usize> = (0..n).filter(|&i| waiting_on[i] == 0).collect();
    while let Some(next) = ready.first().copied() {
        ready.remove(0);
        order.push(next);
        for &r in &edges[next] {
            waiting_on[r] -= 1;
            if waiting_on[r] == 0 {
                ready.push(r);
                ready.sort_unstable();
            }
        }
    }
    if order.len() == n {
        return Ok(order);
    }

    // Everything left is on or downstream of a cycle. Each such track is still
    // waiting on another unfinished one, so walking backwards must repeat a
    // track, and the repeated stretch is the cycle.
    let stuck: Vec<usize> = (0..n).filter(|i| !order.contains(i)).collect();
    let mut path = vec![stuck[0]];
    loop {
        let here = *path.last().unwrap();
        let before = stuck.iter().copied().find(|&w| edges[w].contains(&here)).unwrap();
        if let Some(at) = path.iter().position(|&p| p == before) {
            let mut cycle = path[at..].to_vec();
            cycle.reverse();
            return Err(cycle);
        }
        path.push(before);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceId {
    track: usize,
    serial: u64,
}

#[derive(Default)]
struct ChainFade {
    previous: Option<Voice>,
    inputs: Vec<ChainInput>,
    frames: usize,
    elapsed: usize,
}

struct Track {
    fade: ChainFade,
    def: TrackDef,
    chain: Option<Voice>,
    voices: Vec<(u64, Voice)>,
    chain_silenced: bool,
    /// How many samples in a row the chain has had no input and put nothing audible out.
    chain_quiet: usize,
}

/// Voices that may sound at once unless told otherwise.
pub const DEFAULT_MAX_VOICES: usize = 256;

/// How a new mixer's track takes over from an old mixer's tracks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Transfer {
    /// Release transferred voices while keeping their processor tails.
    pub release: bool,
    /// The old track whose sounding voices continue on this one.
    pub voices_from: Option<usize>,
    /// The old track whose effect chain, with its state, this one keeps.
    pub chain_from: Option<usize>,
}

/// What each track put out, and what each bus carried, kept for an offline render's stems:
/// one buffer per channel for each, grown block by block.
#[derive(Default)]
pub struct Stems {
    pub tracks: Vec<Vec<Vec<f32>>>,
    pub buses: Vec<Vec<Vec<f32>>>,
}

pub struct Mixer {
    stems: Option<Stems>,
    /// The audio input for the block being mixed, one buffer per channel.
    audio_in: Vec<[f32; BLOCK]>,
    buses: Buses,
    tracks: Vec<Track>,
    order: Vec<usize>,
    channels: usize,
    bus_channels: Vec<usize>,
    bus_meters: Vec<(usize, Arc<Meter>)>,
    sample_rate: f32,
    next_serial: u64,
    silenced: usize,
    max_voices: usize,
    stolen: usize,
    muted: bool,
    /// Block-sized working buffers for `process_inner`, kept so they are not cleared again every block.
    summed: Vec<[f32; BLOCK]>,
    scratch: Vec<[f32; BLOCK]>,
    processed: Vec<[f32; BLOCK]>,
}

/// How long a track's effect chain must have had nothing going in and nothing coming out before it is
/// left alone until something arrives, in seconds.
const CHAIN_REST_SECONDS: f32 = 0.1;

impl Mixer {
    /// `bus_channels` gives the channel count of each bus; voices and the master
    /// have `channels`. A track's chain must produce as many channels as its
    /// destination has.
    pub fn new(
        defs: Vec<TrackDef>,
        bus_channels: Vec<usize>,
        channels: usize,
        sample_rate: f32,
    ) -> Result<Mixer, Vec<usize>> {
        assert!((1..=MAX_CHANNELS).contains(&channels), "unsupported channel count {channels}");
        let order = execution_order(&defs)?;
        for def in &defs {
            let wanted = match def.route {
                Route::Master => channels,
                Route::Bus(b) => bus_channels[b],
            };
            let got = def.chain.as_ref().map_or(channels, |g| g.channels());
            assert_eq!(got, wanted, "a track produces {got} channels but its destination has {wanted}");
        }
        let tracks = defs
            .into_iter()
            .map(|def| Track {
                fade: ChainFade { frames: (def.transition.1 * f64::from(sample_rate)).round() as usize, ..Default::default() },
                chain: def.chain.as_ref().map(|g| Voice::new(g.clone(), sample_rate)),
                def,
                voices: Vec::new(),
                chain_silenced: false,
                chain_quiet: 0,
            })
            .collect();
        Ok(Mixer {
            stems: None,
            audio_in: vec![[0.0; BLOCK]; MAX_CHANNELS],
            buses: Buses::new(&bus_channels),
            tracks,
            order,
            channels,
            bus_channels,
            bus_meters: Vec::new(),
            sample_rate,
            next_serial: 0,
            silenced: 0,
            max_voices: DEFAULT_MAX_VOICES,
            stolen: 0,
            muted: false,
            summed: vec![[0.0; BLOCK]; MAX_CHANNELS],
            scratch: vec![[0.0; BLOCK]; MAX_CHANNELS],
            processed: vec![[0.0; BLOCK]; MAX_CHANNELS],
        })
    }

    pub fn set_bus_meters(&mut self, meters: Vec<(usize, Arc<Meter>)>) { self.bus_meters = meters; }

    /// Where the session's clock is at the start of the next block, for `clock.time`.
    pub fn set_clock(&mut self, clock: crate::Clock) {
        self.buses.clock = clock;
    }

    /// How long a sounding note is scheduled to last; see [`Voice::set_dur`].
    pub fn set_voice_dur(&mut self, id: VoiceId, seconds: f32) {
        if let Some(track) = self.tracks.get_mut(id.track)
            && let Some((_, voice)) = track.voices.iter_mut().find(|(serial, _)| *serial == id.serial)
        {
            voice.set_dur(seconds);
        }
    }

    /// The audio input for the next block: `samples` is channel `channel` of it. A channel nobody sets
    /// is silent, and what is set stays until it is set again.
    pub fn set_audio_input(&mut self, channel: usize, samples: &[f32]) {
        if let Some(buffer) = self.audio_in.get_mut(channel) {
            let n = samples.len().min(BLOCK);
            buffer[..n].copy_from_slice(&samples[..n]);
            buffer[n..].fill(0.0);
        }
    }

    /// Reserve bounded voice storage before handing the mixer to rendering.
    pub fn prepare_realtime(&mut self, capacity: usize) {
        for track in &mut self.tracks { track.voices.reserve(capacity); }
    }

    /// Admit an already constructed voice without allocating. Capacity is a
    /// hard limit; rejected voices remain owned by the caller for retirement.
    // the voice comes back in the error so that its owner can retire it without allocating
    #[allow(clippy::result_large_err)]
    pub fn note_prepared(&mut self, track: usize, voice: Voice) -> Result<VoiceId, Voice> {
        if self.muted || track >= self.tracks.len() || voice.channels() != self.channels
            || self.tracks[track].voices.len() == self.tracks[track].voices.capacity()
        { return Err(voice); }
        if let Some(group) = voice.group() {
            for (_, other) in self.tracks.iter_mut().flat_map(|t| t.voices.iter_mut()) {
                if other.group() == Some(group) { other.choke(); }
            }
        }
        let mut voice = voice;
        if let Some(rule) = voice.rule() {
            self.glide_into(track, &rule, &mut voice);
            self.make_room(track, &rule);
        }
        self.steal_if_full();
        let serial = self.next_serial;
        self.next_serial += 1;
        voice.start_note();
        if voice.rule().is_some_and(|r| r.legato) { voice.hold(serial); }
        self.tracks[track].voices.push((serial, voice));
        Ok(VoiceId { track, serial })
    }

    /// Immediate panic without destroying owned resources. A prepared clean
    /// mixer replaces this one, and the old mixer is freed in the background.
    pub fn silence(&mut self) { self.muted = true; }

    /// Start a note on `track`, setting `(parameter index, value)` pairs. The
    /// graph must produce as many channels as the mixer's master has.
    ///
    /// A note in a choke `group` ends every sounding note in the same group
    /// (on any track), with a short fade.
    pub fn note_on(&mut self, track: usize, graph: Arc<Graph>, params: &[(usize, f32)], group: Option<u32>) -> VoiceId {
        self.note_on_with(track, graph, params, group, None)
    }

    /// Like [`Mixer::note_on`], for an instrument that limits its own voices.
    pub fn note_on_with(&mut self, track: usize, graph: Arc<Graph>, params: &[(usize, f32)], group: Option<u32>, rule: Option<VoiceRule>) -> VoiceId {
        assert_eq!(graph.channels(), self.channels, "an instrument must output the master's channel count");
        if group.is_some() {
            for (_, v) in self.tracks.iter_mut().flat_map(|t| t.voices.iter_mut()) {
                if v.group() == group {
                    v.choke();
                }
            }
        }
        let mut voice = Voice::new(graph, self.sample_rate);
        voice.set_group(group);
        voice.set_rule(rule);
        for &(index, value) in params {
            voice.set_param(index, value);
        }
        if let Some(rule) = &rule {
            if let Some(id) = self.merge_legato(track, &voice) { return id; }
            self.glide_into(track, rule, &mut voice);
            self.make_room(track, rule);
        }
        self.steal_if_full();
        let serial = self.next_serial;
        self.next_serial += 1;
        voice.start_note();
        if rule.is_some_and(|r| r.legato) { voice.hold(serial); }
        self.tracks[track].voices.push((serial, voice));
        VoiceId { track, serial }
    }

    /// A legato note that starts while its instrument's voice is held takes that voice over, so the
    /// envelopes carry on and only the pitch moves. The `note` it came from is only read, so a caller
    /// that must not free memory can retire it afterwards. Allocates nothing.
    pub fn merge_legato(&mut self, track: usize, note: &Voice) -> Option<VoiceId> {
        let rule = note.rule().filter(|r| r.legato && !self.muted)?;
        let serial = self.next_serial;
        let (_, voice) = self.tracks.get_mut(track)?.voices.iter_mut()
            .find(|(_, v)| v.is_legato_open() && v.rule().is_some_and(|r| r.instrument == rule.instrument))?;
        voice.legato_on(serial, note);
        self.next_serial += 1;
        Some(VoiceId { track, serial })
    }

    /// A glissando into `voice`: it starts at the pitch the instrument's newest note had reached.
    fn glide_into(&self, track: usize, rule: &VoiceRule, voice: &mut Voice) {
        if rule.glide <= 0.0 { return; }
        let Some(track) = self.tracks.get(track) else { return };
        let previous = track.voices.iter()
            .filter(|(_, v)| !v.is_choked() && v.rule().is_some_and(|r| r.instrument == rule.instrument))
            .max_by_key(|(serial, _)| *serial);
        if let Some((_, v)) = previous { voice.glide_from(v.pitch_now(), self.sample_rate); }
    }

    /// Make room for a note under `rule`: release the sounding note of the same pitch if asked, and
    /// end the oldest notes of the instrument until fewer than `max` are left. Allocates nothing.
    fn make_room(&mut self, track: usize, rule: &VoiceRule) {
        let Some(track) = self.tracks.get_mut(track) else { return };
        let mine = |v: &Voice| v.rule().is_some_and(|r| r.instrument == rule.instrument);
        if rule.release_same_pitch {
            for (_, v) in track.voices.iter_mut() {
                if mine(v) && !v.is_released() && v.rule().is_some_and(|r| r.pitch == rule.pitch) { v.release(); }
            }
        }
        loop {
            // the voice to end: the quietest (the oldest of equals), or just the oldest
            let (mut count, mut ends) = (0usize, None::<(u64, f32)>);
            for (serial, v) in track.voices.iter() {
                if mine(v) && !v.is_choked() {
                    count += 1;
                    let level = if rule.steal_quietest { v.level() } else { 0.0 };
                    if ends.is_none_or(|(s, l)| level < l || (level == l && *serial < s)) { ends = Some((*serial, level)); }
                }
            }
            let Some((ends, _)) = ends.filter(|_| count >= rule.max.max(1)) else { break };
            if let Some((_, v)) = track.voices.iter_mut().find(|(serial, _)| *serial == ends) { v.choke(); }
        }
    }

    pub fn set_control(&mut self, key: &str, start: f32, target: f32, frames: u64, elapsed: u64) {
        for track in &mut self.tracks {
            for (_, voice) in &mut track.voices {
                voice.set_control(key, start, target, frames, elapsed);
            }
            if let Some(chain) = &mut track.chain {
                let local = track
                    .def
                    .controls
                    .iter()
                    .find(|(public, _)| public == key)
                    .map_or(key, |(_, local)| local.as_str());
                // A template's private controls are changed through its instance keys only.
                if !track
                    .def
                    .controls
                    .iter()
                    .any(|(_, template)| template == key)
                    || local != key
                {
                    chain.set_control(local, start, target, frames, elapsed);
                }
            }
        }
    }

    /// Place a sounding note among the speakers; see [`Voice::set_pan`].
    pub fn set_voice_pan(&mut self, id: VoiceId, gains: Option<[f32; MAX_CHANNELS]>) {
        if let Some(track) = self.tracks.get_mut(id.track)
            && let Some((_, voice)) = track.voices.iter_mut().find(|(serial, _)| *serial == id.serial)
        {
            voice.set_pan(gains);
        }
    }

    pub fn set_voice_control(&mut self, id: VoiceId, key: &str, start: f32, target: f32, frames: u64, elapsed: u64) {
        if let Some(track) = self.tracks.get_mut(id.track)
            && let Some((_, voice)) = track.voices.iter_mut().find(|(serial, _)| *serial == id.serial)
        { voice.set_control(key, start, target, frames, elapsed); }
    }

    /// Release a note. A voice that has already finished is ignored. Voices keep
    /// their id when they move between tracks (see [`Mixer::adopt`]), so the
    /// track it started on is only a hint.
    pub fn release(&mut self, id: VoiceId) {
        let ends = |(s, v): &(u64, Voice)| *s == id.serial || v.holds(id.serial);
        let hinted = self.tracks.get_mut(id.track).and_then(|t| t.voices.iter_mut().find(|n| ends(n)));
        if let Some((_, v)) = hinted {
            v.note_off(id.serial);
            return;
        }
        for track in &mut self.tracks {
            if let Some((_, v)) = track.voices.iter_mut().find(|n| ends(n)) {
                v.note_off(id.serial);
                return;
            }
        }
    }

    /// Set a parameter of a sounding note by name, as a keyboard's pitch bend does. A note that has
    /// ended, or an instrument without the parameter, ignores it. Allocates nothing.
    pub fn set_voice_param(&mut self, id: VoiceId, name: &str, value: f32) {
        let ends = |(s, v): &(u64, Voice)| *s == id.serial || v.holds(id.serial);
        for track in &mut self.tracks {
            if let Some((_, v)) = track.voices.iter_mut().find(|n| ends(n)) {
                v.set_param_by_name(name, value);
                return;
            }
        }
    }

    /// Move a parameter of a sounding note, by its index, to `value` over `frames` (at once if 0). A note
    /// that has ended is ignored. Allocates nothing.
    pub fn set_voice_param_index(&mut self, id: VoiceId, index: usize, value: f32, frames: u64) {
        let ends = |(s, v): &(u64, Voice)| *s == id.serial || v.holds(id.serial);
        for track in &mut self.tracks {
            if let Some((_, v)) = track.voices.iter_mut().find(|n| ends(n)) {
                v.set_param_glide(index, value, frames);
                return;
            }
        }
    }

    pub fn swap_voice_mapping(&mut self, id: VoiceId, name: &str, mapping: &mut Option<Box<crate::ParamMapping>>) {
        let ends = |(s,v): &(u64,Voice)| *s == id.serial || v.holds(id.serial);
        for track in &mut self.tracks {
            if let Some((_,voice)) = track.voices.iter_mut().find(|v| ends(v)) { voice.swap_mapping(name,mapping); return; }
        }
    }

    pub fn set_voice_param_glide(&mut self, id: VoiceId, name: &str, value: f32, frames: u64) {
        let ends = |(s,v): &(u64,Voice)| *s == id.serial || v.holds(id.serial);
        for track in &mut self.tracks {
            if let Some((_,voice)) = track.voices.iter_mut().find(|v| ends(v)) {
                if let Some(index) = voice.param_index(name) { voice.set_param_glide(index,value,frames); } return;
            }
        }
    }

    /// Silence everything at once: stop every voice, clear every effect's
    /// memory (delay lines, reverb tails) and empty the buses.
    pub fn panic(&mut self) {
        let rate = self.sample_rate;
        for track in &mut self.tracks {
            track.voices.clear();
            track.fade = ChainFade::default();
            track.chain = track.def.chain.as_ref().map(|g| Voice::new(g.clone(), rate));
            track.chain_silenced = false;
            track.chain_quiet = 0;
        }
        self.buses.clear();
        self.muted = false;
    }

    /// Start keeping what every track puts out (after its effects, before the master) and what every
    /// bus carries. Only for offline renders: it grows without bound.
    pub fn capture_stems(&mut self) {
        self.stems = Some(Stems { tracks: vec![Vec::new(); self.tracks.len()], buses: vec![Vec::new(); self.bus_channels.len()] });
    }

    /// Fill the captured stems with silence up to `len` samples each, for stretches the caller skipped
    /// mixing because nothing could be heard.
    pub fn pad_stems(&mut self, len: usize) {
        if let Some(stems) = &mut self.stems {
            for channel in stems.tracks.iter_mut().chain(stems.buses.iter_mut()).flatten() {
                if channel.len() < len { channel.resize(len, 0.0); }
            }
        }
    }

    /// The stems captured so far, which are no longer kept.
    pub fn take_stems(&mut self) -> Option<Stems> {
        self.stems.take()
    }

    /// The most voices that may sound at once. A new note past this ends the
    /// oldest one that is still going, with a short fade.
    pub fn set_max_voices(&mut self, max: usize) {
        self.max_voices = max.max(1);
    }

    /// How many voices were ended early for lack of room since this was last called.
    pub fn take_stolen(&mut self) -> usize {
        std::mem::take(&mut self.stolen)
    }

    fn steal_if_full(&mut self) {
        let count = self.active_voices();
        if count < self.max_voices {
            return;
        }
        let oldest = self
            .tracks
            .iter_mut()
            .flat_map(|t| t.voices.iter_mut())
            .filter(|(_, v)| !v.is_choked())
            .min_by_key(|(serial, _)| *serial);
        if let Some((_, voice)) = oldest {
            voice.choke();
            self.stolen += 1;
        }
    }

    pub fn active_voices(&self) -> usize {
        self.tracks.iter().map(|t| t.voices.len()).sum()
    }

    /// How many voices and chains were silenced for producing NaN or infinity
    /// since this was last called.
    pub fn take_silenced(&mut self) -> usize {
        std::mem::take(&mut self.silenced)
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Mix `frames` samples (at most [`BLOCK`]) of everything into `out`, one
    /// block per master channel.
    pub fn process(&mut self, out: &mut [[f32; BLOCK]], frames: usize) {
        self.process_inner(out, frames, None);
    }

    /// Render while retaining finished voices for off-thread destruction.
    /// `retired` must have been reserved by the caller before rendering.
    pub fn process_retained(&mut self, out: &mut [[f32; BLOCK]], frames: usize, retired: &mut Vec<Voice>) {
        self.process_inner(out, frames, Some(retired));
    }

    fn process_inner(&mut self, out: &mut [[f32; BLOCK]], frames: usize, mut retired: Option<&mut Vec<Voice>>) {
        assert!(frames <= BLOCK, "block too long: {frames} > {BLOCK}");
        assert_eq!(out.len(), self.channels, "one output block per master channel");
        // every way of rendering audio comes through here, so they all treat tiny numbers alike
        let _flush = crate::denormals::FlushToZero::new();
        out.iter_mut().for_each(|c| c[..frames].fill(0.0));
        if self.muted {
            self.buses.clear();
            return;
        }
        self.buses.begin_block(frames);

        let n = self.channels;
        let (summed, scratch, processed) = (&mut self.summed, &mut self.scratch, &mut self.processed);
        let rest_after = (CHAIN_REST_SECONDS * self.sample_rate) as usize;

        for &t in &self.order {
            let track = &mut self.tracks[t];
            if track.fade.elapsed >= track.fade.frames && track.fade.previous.is_some() {
                if let Some(retired) = retired.as_deref_mut() {
                    if retired.len() < retired.capacity() { retired.push(track.fade.previous.take().unwrap()); }
                } else { track.fade.previous = None; }
            }

            summed[..n].iter_mut().for_each(|c| c[..frames].fill(0.0));
            for (_, voice) in &mut track.voices {
                voice.process_blocks(&mut scratch[..n], frames, &mut self.buses);
                for (sum, voice_out) in summed[..n].iter_mut().zip(&scratch[..n]) {
                    for (s, v) in sum[..frames].iter_mut().zip(&voice_out[..frames]) {
                        *s += v;
                    }
                }
            }
            if let Some(retired) = retired.as_deref_mut() {
                let mut i = 0;
                while i < track.voices.len() {
                    if track.voices[i].1.is_finished() && retired.len() < retired.capacity() {
                        let (_, voice) = track.voices.remove(i);
                        self.silenced += usize::from(voice.is_poisoned());
                        retired.push(voice);
                    } else { i += 1; }
                }
            } else {
                self.silenced += track.voices.iter().filter(|(_, v)| v.is_poisoned()).count();
                track.voices.retain(|(_, v)| !v.is_finished());
            }

            let result: &[[f32; BLOCK]] = match &mut track.chain {
                None => &summed[..n],
                Some(chain) => {
                    let out_channels = chain.channels();
                    let heard = |input: &ChainInput| -> &[f32] {
                        match *input {
                            ChainInput::It { channel } => &summed[channel][..frames],
                            ChainInput::Audio { channel } => &self.audio_in[channel][..frames],
                            ChainInput::Bus { bus, channel } => self.buses.get(bus, channel, frames),
                        }
                    };
                    let silent_in = track.def.inputs.iter().all(|input| heard(input).iter().all(|s| *s == 0.0));
                    if silent_in && track.chain_quiet >= rest_after && !(track.fade.previous.is_some() && track.fade.elapsed < track.fade.frames) {
                        // nothing to ring out and nothing coming in: the chain stays as it is, and says nothing
                        processed[..out_channels].iter_mut().for_each(|c| c[..frames].fill(0.0));
                    } else {
                        for (i, input) in track.def.inputs.iter().enumerate() {
                            chain.set_input(i, heard(input));
                        }
                        if let Some(previous) = &mut track.fade.previous { for (i,input) in track.fade.inputs.iter().enumerate() { previous.set_input(i, heard(input)); } }
                        chain.process_blocks(&mut processed[..out_channels], frames, &mut self.buses);
                        if track.fade.elapsed < track.fade.frames && let Some(previous) = &mut track.fade.previous {
                            previous.process_blocks(&mut scratch[..out_channels], frames, &mut self.buses);
                            for (next,old) in processed[..out_channels].iter_mut().zip(&scratch[..out_channels]) {
                                for i in 0..frames {
                                    let mix = ((track.fade.elapsed + i) as f32 / track.fade.frames as f32).min(1.0);
                                    next[i] = old[i] * (1.0-mix) + next[i] * mix;
                                }
                            }
                            track.fade.elapsed = track.fade.elapsed.saturating_add(frames);
                        }
                        if chain.is_poisoned() && !track.chain_silenced {
                            track.chain_silenced = true;
                            self.silenced += 1;
                        }
                        track.chain_quiet = if silent_in && chain.is_quiet(frames) { track.chain_quiet + frames } else { 0 };
                    }
                    &processed[..out_channels]
                }
            };

            if let Some(meter) = &track.def.meter { meter.observe(result,frames); }
            if let Some(stems) = &mut self.stems {
                keep(&mut stems.tracks[t], result, frames);
            }
            match track.def.route {
                Route::Master => {
                    for (o, r) in out.iter_mut().zip(result) {
                        for (d, s) in o[..frames].iter_mut().zip(&r[..frames]) {
                            *d += s;
                        }
                    }
                }
                Route::Bus(b) => {
                    for (c, r) in result.iter().enumerate() {
                        self.buses.add(b, c, &r[..frames]);
                    }
                }
            }
        }
        for (bus,meter) in &self.bus_meters { meter.observe(&self.buses.data[*bus], frames); }
        self.buses.end_block(frames);
        if let Some(stems) = &mut self.stems {
            for (bus, kept) in stems.buses.iter_mut().enumerate() {
                let channels = self.bus_channels[bus];
                kept.resize(channels, Vec::new());
                for (c, channel) in kept.iter_mut().enumerate() {
                    channel.extend_from_slice(self.buses.get(bus, c, frames));
                }
            }
        }
    }

    /// Update sounding voices' sends before transferring them to a mixer with
    /// a changed bus list. The graphs stay on their original definitions.
    pub fn remap_buses(&mut self, mapping: &[Option<usize>]) {
        self.remap_buses_with(&mut |i| mapping.get(i).copied().flatten());
    }

    pub fn remap_buses_with(&mut self, mapping: &mut impl FnMut(usize) -> Option<usize>) {
        for track in &mut self.tracks {
            for (_, voice) in &mut track.voices { voice.remap_buses_with(mapping); }
            for input in &mut track.def.inputs { if let ChainInput::Bus { bus, .. } = input && let Some(next) = mapping(*bus) { *bus = next; } }
            if let Some(previous) = &mut track.fade.previous {
                previous.remap_buses_with(mapping);
                for input in &mut track.fade.inputs { if let ChainInput::Bus { bus, .. } = input && let Some(next) = mapping(*bus) { *bus = next; } }
            }
        }
    }

    /// Take over running state from `old`, which this mixer replaces: for each
    /// of this mixer's tracks, `transfers` says which of the old tracks' voices
    /// to continue with and whose effect chain (with its delay lines and filter
    /// memory) to keep. Voices keep playing on the instrument definition they
    /// started with. Old tracks that nothing claims are dropped.
    ///
    /// Chains carry memory at compatible nodes into the new definition.
    /// The caller must only carry state where bus identities are unchanged.
    pub fn adopt(&mut self, old: &mut Mixer, transfers: &[Transfer]) {
        assert_eq!(transfers.len(), self.tracks.len(), "one transfer per track");
        for (track, transfer) in self.tracks.iter_mut().zip(transfers) {
            if let Some(from) = transfer.voices_from
                && old.channels == self.channels
            {
                track.voices.append(&mut old.tracks[from].voices);
                if transfer.release {
                    for (_, voice) in &mut track.voices {
                        voice.release();
                    }
                }
            }
            if let Some(from) = transfer.chain_from {
                let previous = &mut old.tracks[from];
                let changed = track.def.transition.0 != previous.def.transition.0;
                if changed && track.fade.frames > 0 {
                    // a different DSP: fade it in from the chain that was playing
                    track.fade.previous = previous.chain.take();
                    track.fade.inputs = std::mem::take(&mut previous.def.inputs);
                } else {
                    // the same DSP, or one that cannot crossfade: its memory comes straight across
                    if let (Some(chain),Some(old_chain)) = (&mut track.chain,&mut previous.chain) { chain.carry_state(old_chain); }
                    if !changed { std::mem::swap(&mut track.fade, &mut previous.fade); }
                }
            }
        }
        self.next_serial = self.next_serial.max(old.next_serial);
        self.silenced += std::mem::take(&mut old.silenced);
        // stems being kept carry on, if the new mixer has the same tracks and buses
        if let Some(stems) = old.stems.take_if(|s| s.tracks.len() == self.tracks.len() && s.buses.len() == self.bus_channels.len()) {
            self.stems = Some(stems);
        }
    }

    /// The channel count of each bus.
    pub fn bus_channels(&self) -> &[usize] {
        &self.bus_channels
    }
}

/// Append the first `frames` samples of every channel of `block` to `kept`.
fn keep(kept: &mut Vec<Vec<f32>>, block: &[[f32; BLOCK]], frames: usize) {
    kept.resize(block.len(), Vec::new());
    for (channel, samples) in kept.iter_mut().zip(block) {
        channel.extend_from_slice(&samples[..frames]);
    }
}
