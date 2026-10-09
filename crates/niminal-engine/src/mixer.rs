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

/// Block-sized buffers that are summed into and cleared every block.
pub struct Buses {
    data: Vec<[f32; BLOCK]>,
}

impl Buses {
    pub fn new(count: usize) -> Self {
        Buses { data: vec![[0.0; BLOCK]; count] }
    }

    pub fn clear(&mut self) {
        self.data.iter_mut().for_each(|b| *b = [0.0; BLOCK]);
    }

    /// Add `samples` to the start of a bus. A bus that doesn't exist is ignored.
    pub fn add(&mut self, bus: usize, samples: &[f32]) {
        if let Some(b) = self.data.get_mut(bus) {
            for (d, s) in b.iter_mut().zip(samples) {
                *d += s;
            }
        }
    }

    pub fn get(&self, bus: usize, frames: usize) -> &[f32] {
        &self.data[bus][..frames]
    }
}

/// Where a track's output goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Master,
    Bus(usize),
}

/// What feeds one of a track chain's external inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainInput {
    /// The summed output of the track's voices.
    It,
    Bus(usize),
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
                ChainInput::Bus(b) => Some(*b),
                ChainInput::It => None,
            })
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

pub struct Mixer {
    buses: Buses,
    tracks: Vec<Track>,
    order: Vec<usize>,
    sample_rate: f32,
    next_serial: u64,
    silenced: usize,
}

impl Mixer {
    pub fn new(defs: Vec<TrackDef>, bus_count: usize, sample_rate: f32) -> Result<Mixer, Vec<usize>> {
        let order = execution_order(&defs)?;
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
            buses: Buses::new(bus_count),
            tracks,
            order,
            sample_rate,
            next_serial: 0,
            silenced: 0,
        })
    }

    /// Start a note on `track`, setting `(parameter index, value)` pairs.
    pub fn note_on(&mut self, track: usize, graph: Arc<Graph>, params: &[(usize, f32)]) -> VoiceId {
        let mut voice = Voice::new(graph, self.sample_rate);
        for &(index, value) in params {
            voice.set_param(index, value);
        }
        let serial = self.next_serial;
        self.next_serial += 1;
        self.tracks[track].voices.push((serial, voice));
        VoiceId { track, serial }
    }

    /// Release a note. A voice that has already finished is ignored.
    pub fn release(&mut self, id: VoiceId) {
        if let Some((_, v)) = self.tracks[id.track].voices.iter_mut().find(|(s, _)| *s == id.serial) {
            v.release();
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

    /// Mix `out.len()` samples (at most [`BLOCK`]) of everything to the master.
    pub fn process(&mut self, out: &mut [f32]) {
        let frames = out.len();
        assert!(frames <= BLOCK, "block too long: {frames} > {BLOCK}");
        out.fill(0.0);
        self.buses.clear();

        let mut summed = [0.0f32; BLOCK];
        let mut scratch = [0.0f32; BLOCK];
        let mut processed = [0.0f32; BLOCK];

        for &t in &self.order {
            let track = &mut self.tracks[t];

            summed[..frames].fill(0.0);
            for (_, voice) in &mut track.voices {
                voice.process_routed(&mut scratch[..frames], &mut self.buses);
                for (s, v) in summed[..frames].iter_mut().zip(&scratch[..frames]) {
                    *s += v;
                }
            }
            self.silenced += track.voices.iter().filter(|(_, v)| v.is_poisoned()).count();
            track.voices.retain(|(_, v)| !v.is_finished());

            let result: &[f32] = match &mut track.chain {
                None => &summed[..frames],
                Some(chain) => {
                    for (i, input) in track.def.inputs.iter().enumerate() {
                        match input {
                            ChainInput::It => chain.set_input(i, &summed[..frames]),
                            ChainInput::Bus(b) => chain.set_input(i, self.buses.get(*b, frames)),
                        }
                    }
                    chain.process_routed(&mut processed[..frames], &mut self.buses);
                    if chain.is_poisoned() && !track.chain_silenced {
                        track.chain_silenced = true;
                        self.silenced += 1;
                    }
                    &processed[..frames]
                }
            };

            match track.def.route {
                Route::Master => out.iter_mut().zip(result).for_each(|(o, r)| *o += r),
                Route::Bus(b) => self.buses.add(b, result),
            }
        }
    }
}
