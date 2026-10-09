//! Tracks, buses and the master: everything between voices and the speakers.
//!
//! Each block, buses are cleared; then tracks run in dependency order, so
//! everything that writes a bus runs before anything that reads it. That is
//! why a send reaches its reader in the same block, with no added latency.
//! The order is worked out from the graphs themselves, so the order tracks are
//! declared in never matters.

use std::sync::Arc;

use crate::graph::Graph;
use crate::opcode::BLOCK;
use crate::voice::Voice;

/// The most channels any signal path may carry (7.1.4 needs 12).
pub const MAX_CHANNELS: usize = 16;

/// Block-sized, possibly multichannel buffers that are summed into and cleared
/// every block.
pub struct Buses {
    data: Vec<Vec<[f32; BLOCK]>>,
}

impl Buses {
    /// One bus per entry, with that many channels.
    pub fn new(channels: &[usize]) -> Self {
        Buses { data: channels.iter().map(|&n| vec![[0.0; BLOCK]; n]).collect() }
    }

    pub fn clear(&mut self) {
        for bus in &mut self.data {
            bus.iter_mut().for_each(|c| *c = [0.0; BLOCK]);
        }
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

    pub fn get(&self, bus: usize, channel: usize, frames: usize) -> &[f32] {
        &self.data[bus][channel][..frames]
    }

    pub fn channels(&self, bus: usize) -> usize {
        self.data[bus].len()
    }
}

/// Where a track's output goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
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
}

#[derive(Clone)]
pub struct TrackDef {
    /// Processing applied to the summed voices. `None` passes them through.
    pub chain: Option<Arc<Graph>>,
    /// What each of the chain graph's inputs receives, in input order.
    pub inputs: Vec<ChainInput>,
    pub route: Route,
    /// Buses that this track's voices may send to.
    pub voice_sends: Vec<usize>,
}

impl TrackDef {
    fn reads(&self) -> Vec<usize> {
        self.inputs
            .iter()
            .filter_map(|i| match i {
                ChainInput::Bus { bus, .. } => Some(*bus),
                ChainInput::It { .. } => None,
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
}

/// An order in which to run tracks so that every bus is written before it is
/// read. A cycle comes back as the tracks that form it, in order; a track that
/// both writes and reads one bus is a cycle of one.
pub fn execution_order(tracks: &[TrackDef]) -> Result<Vec<usize>, Vec<usize>> {
    let n = tracks.len();
    let reads: Vec<_> = tracks.iter().map(TrackDef::reads).collect();
    let writes: Vec<_> = tracks.iter().map(TrackDef::writes).collect();
    // edges[w] lists tracks that must run after w
    let edges: Vec<Vec<usize>> = (0..n)
        .map(|w| (0..n).filter(|&r| writes[w].iter().any(|b| reads[r].contains(b))).collect())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceId {
    track: usize,
    serial: u64,
}

struct Track {
    def: TrackDef,
    chain: Option<Voice>,
    voices: Vec<(u64, Voice)>,
    chain_silenced: bool,
}

/// Voices that may sound at once unless told otherwise.
pub const DEFAULT_MAX_VOICES: usize = 256;

/// How a new mixer's track takes over from an old mixer's tracks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Transfer {
    /// The old track whose sounding voices continue on this one.
    pub voices_from: Option<usize>,
    /// The old track whose effect chain, with its state, this one keeps.
    pub chain_from: Option<usize>,
}

pub struct Mixer {
    buses: Buses,
    tracks: Vec<Track>,
    order: Vec<usize>,
    channels: usize,
    bus_channels: Vec<usize>,
    sample_rate: f32,
    next_serial: u64,
    silenced: usize,
    max_voices: usize,
    stolen: usize,
}

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
                chain: def.chain.as_ref().map(|g| Voice::new(g.clone(), sample_rate)),
                def,
                voices: Vec::new(),
                chain_silenced: false,
            })
            .collect();
        Ok(Mixer {
            buses: Buses::new(&bus_channels),
            tracks,
            order,
            channels,
            bus_channels,
            sample_rate,
            next_serial: 0,
            silenced: 0,
            max_voices: DEFAULT_MAX_VOICES,
            stolen: 0,
        })
    }

    /// Start a note on `track`, setting `(parameter index, value)` pairs. The
    /// graph must produce as many channels as the mixer's master has.
    ///
    /// A note in a choke `group` ends every sounding note in the same group
    /// (on any track), with a short fade.
    pub fn note_on(&mut self, track: usize, graph: Arc<Graph>, params: &[(usize, f32)], group: Option<u32>) -> VoiceId {
        assert_eq!(graph.channels(), self.channels, "an instrument must output the master's channel count");
        if group.is_some() {
            for (_, v) in self.tracks.iter_mut().flat_map(|t| t.voices.iter_mut()) {
                if v.group() == group {
                    v.choke();
                }
            }
        }
        self.steal_if_full();
        let mut voice = Voice::new(graph, self.sample_rate);
        voice.set_group(group);
        for &(index, value) in params {
            voice.set_param(index, value);
        }
        let serial = self.next_serial;
        self.next_serial += 1;
        self.tracks[track].voices.push((serial, voice));
        VoiceId { track, serial }
    }

    /// Release a note. A voice that has already finished is ignored. Voices keep
    /// their id when they move between tracks (see [`Mixer::adopt`]), so the
    /// track it started on is only a hint.
    pub fn release(&mut self, id: VoiceId) {
        let hinted = self.tracks.get_mut(id.track).and_then(|t| t.voices.iter_mut().find(|(s, _)| *s == id.serial));
        if let Some((_, v)) = hinted {
            v.release();
            return;
        }
        for track in &mut self.tracks {
            if let Some((_, v)) = track.voices.iter_mut().find(|(s, _)| *s == id.serial) {
                v.release();
                return;
            }
        }
    }

    /// Silence everything at once: stop every voice, clear every effect's
    /// memory (delay lines, reverb tails) and empty the buses.
    pub fn panic(&mut self) {
        let rate = self.sample_rate;
        for track in &mut self.tracks {
            track.voices.clear();
            track.chain = track.def.chain.as_ref().map(|g| Voice::new(g.clone(), rate));
            track.chain_silenced = false;
        }
        self.buses.clear();
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
        assert!(frames <= BLOCK, "block too long: {frames} > {BLOCK}");
        assert_eq!(out.len(), self.channels, "one output block per master channel");
        out.iter_mut().for_each(|c| c[..frames].fill(0.0));
        self.buses.clear();

        let n = self.channels;
        let mut summed = [[0.0f32; BLOCK]; MAX_CHANNELS];
        let mut scratch = [[0.0f32; BLOCK]; MAX_CHANNELS];
        let mut processed = [[0.0f32; BLOCK]; MAX_CHANNELS];

        for &t in &self.order {
            let track = &mut self.tracks[t];

            summed[..n].iter_mut().for_each(|c| c[..frames].fill(0.0));
            for (_, voice) in &mut track.voices {
                voice.process_blocks(&mut scratch[..n], frames, &mut self.buses);
                for (sum, voice_out) in summed[..n].iter_mut().zip(&scratch[..n]) {
                    for (s, v) in sum[..frames].iter_mut().zip(&voice_out[..frames]) {
                        *s += v;
                    }
                }
            }
            self.silenced += track.voices.iter().filter(|(_, v)| v.is_poisoned()).count();
            track.voices.retain(|(_, v)| !v.is_finished());

            let result: &[[f32; BLOCK]] = match &mut track.chain {
                None => &summed[..n],
                Some(chain) => {
                    for (i, input) in track.def.inputs.iter().enumerate() {
                        match *input {
                            ChainInput::It { channel } => chain.set_input(i, &summed[channel][..frames]),
                            ChainInput::Bus { bus, channel } => {
                                chain.set_input(i, self.buses.get(bus, channel, frames));
                            }
                        }
                    }
                    let out_channels = chain.channels();
                    chain.process_blocks(&mut processed[..out_channels], frames, &mut self.buses);
                    if chain.is_poisoned() && !track.chain_silenced {
                        track.chain_silenced = true;
                        self.silenced += 1;
                    }
                    &processed[..out_channels]
                }
            };

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
    }

    /// Take over running state from `old`, which this mixer replaces: for each
    /// of this mixer's tracks, `transfers` says which of the old tracks' voices
    /// to continue with and whose effect chain (with its delay lines and filter
    /// memory) to keep. Voices keep playing on the instrument definition they
    /// started with. Old tracks that nothing claims are dropped.
    ///
    /// The caller must only carry a chain over where it is the same as the new
    /// definition, and only carry voices where the buses are unchanged.
    pub fn adopt(&mut self, old: &mut Mixer, transfers: &[Transfer]) {
        assert_eq!(transfers.len(), self.tracks.len(), "one transfer per track");
        for (track, transfer) in self.tracks.iter_mut().zip(transfers) {
            if let Some(from) = transfer.voices_from
                && old.channels == self.channels
            {
                track.voices.append(&mut old.tracks[from].voices);
            }
            if let Some(from) = transfer.chain_from
                && let Some(chain) = old.tracks[from].chain.take()
            {
                track.chain = Some(chain);
                track.chain_silenced = old.tracks[from].chain_silenced;
            }
        }
        self.next_serial = self.next_serial.max(old.next_serial);
        self.silenced += std::mem::take(&mut old.silenced);
    }

    /// The channel count of each bus.
    pub fn bus_channels(&self) -> &[usize] {
        &self.bus_channels
    }
}
