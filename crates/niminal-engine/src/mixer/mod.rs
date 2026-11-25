//! Nodes, buses and the master: everything between voices and the speakers.
//!
//! Each block, buses are cleared; then nodes run in dependency order, so
//! everything that writes a bus runs before anything that reads it. That is
//! why a send reaches its reader in the same block, with no added latency.
//! The order is worked out from the graphs themselves, so the order nodes are
//! declared in never matters.

mod bus;
mod graph;
mod render;
mod voices;

pub use bus::{Buses, MAX_CHANNELS, Meter};
pub(crate) use bus::Sends;
pub use graph::{ChainInput, NodeDef, Route, execution_order};
pub use render::{Fork, VOICE_CHUNK};

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use crate::opcode::{block_size, blocks};
use crate::voice::Voice;
use graph::independent_runs;
use render::{LANES, LaneCell};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceId {
    node: usize,
    serial: u64,
}

#[derive(Default)]
struct ChainFade {
    previous: Option<Voice>,
    inputs: Vec<ChainInput>,
    frames: usize,
    elapsed: usize,
}

struct Node {
    fade: ChainFade,
    def: NodeDef,
    chain: Option<Voice>,
    voices: Vec<(u64, Voice)>,
    /// How many voices the node has taken in, which gives each its chunk.
    admitted: u64,
    /// What its voices added up to in the block being mixed.
    summed: Vec<Vec<f32>>,
    /// How long a voice takes to make a frame of sound, in nanoseconds, as timed lately; 0 if never.
    rate: f32,
    /// Nanoseconds spent on its voices in the block being mixed.
    voice_nanos: u64,
    chain_silenced: bool,
    /// How many samples in a row the chain has had no input and put nothing audible out.
    chain_quiet: usize,
}

impl Node {
    /// Take in a voice. Voices are summed in chunks of [`VOICE_CHUNK`] taken in one after another, so
    /// which voices are summed together depends on when notes start and never on when voices end.
    fn admit(&mut self, serial: u64, mut voice: Voice) {
        voice.set_chunk(self.admitted / VOICE_CHUNK as u64);
        self.admitted += 1;
        self.voices.push((serial, voice));
    }
}

/// Voices that may sound at once unless told otherwise.
pub const DEFAULT_MAX_VOICES: usize = 256;

/// How a new mixer's node takes over from an old mixer's nodes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Transfer {
    /// Release transferred voices while keeping their DSP tails.
    pub release: bool,
    /// The old node whose sounding voices continue on this one.
    pub voices_from: Option<usize>,
    /// The old node whose effect chain, with its state, this one keeps.
    pub chain_from: Option<usize>,
}

/// What each node put out, and what each bus carried, kept for an offline render's stems:
/// one buffer per channel for each, grown block by block.
#[derive(Default)]
pub struct Stems {
    pub nodes: Vec<Vec<Vec<f32>>>,
    pub buses: Vec<Vec<Vec<f32>>>,
}

/// How much work each node of a program has been doing, shared with the control thread.
/// Costs are nanoseconds per block of [`BLOCK`] frames, smoothed over recent blocks.
#[derive(Debug, Default)]
pub struct Profile {
    pub nodes: Vec<NodeLoad>,
}

#[derive(Debug, Default)]
pub struct NodeLoad {
    pub voices: AtomicU32,
    /// Making and summing the node's voices, added up over every thread that helped.
    pub voice_nanos: AtomicU32,
    pub chain_nanos: AtomicU32,
}

impl Profile {
    pub fn new(nodes: usize) -> Self {
        Profile {
            nodes: (0..nodes).map(|_| NodeLoad::default()).collect(),
        }
    }
}

impl NodeLoad {
    pub(super) fn observe(&self, voices: usize, voice_nanos: u64, chain_nanos: u64) {
        let smooth = |cell: &AtomicU32, sample: u64| {
            let old = u64::from(cell.load(Ordering::Relaxed));
            let new = old + sample.min(u64::from(u32::MAX)) / 8 - old / 8;
            cell.store(new as u32, Ordering::Relaxed);
        };
        self.voices.store(voices as u32, Ordering::Relaxed);
        smooth(&self.voice_nanos, voice_nanos);
        smooth(&self.chain_nanos, chain_nanos);
    }
}

pub struct Mixer {
    profile: Option<Arc<Profile>>,
    stems: Option<Stems>,
    /// The audio input for the block being mixed, one buffer per channel.
    audio_in: Vec<Vec<f32>>,
    buses: Buses,
    nodes: Vec<Node>,
    order: Vec<usize>,
    /// Runs of nodes in `order` that do not depend on each other, which are rendered side by side.
    groups: Vec<std::ops::Range<usize>>,
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
    scratch: Vec<Vec<f32>>,
    processed: Vec<Vec<f32>>,
    /// Working space for chunks of voices that run side by side, one lane for each.
    lanes: Vec<LaneCell>,
    /// Runs the jobs of a wave of chunks on several threads; without it they run one after another.
    fork: Option<Arc<dyn Fork>>,
}

impl Mixer {
    /// `bus_channels` gives the channel count of each bus; voices and the master
    /// have `channels`. A node's chain must produce as many channels as its
    /// destination has.
    pub fn new(
        defs: Vec<NodeDef>,
        bus_channels: Vec<usize>,
        channels: usize,
        sample_rate: f32,
    ) -> Result<Mixer, Vec<usize>> {
        assert!(
            (1..=MAX_CHANNELS).contains(&channels),
            "unsupported channel count {channels}"
        );
        let order = execution_order(&defs)?;
        let groups = independent_runs(&defs, &order);
        for def in &defs {
            let wanted = match def.route {
                Route::Master => channels,
                Route::Bus(b) => bus_channels[b],
            };
            let got = def.chain.as_ref().map_or(channels, |g| g.channels());
            assert_eq!(
                got, wanted,
                "an fx produces {got} channels but its destination has {wanted}"
            );
        }
        let nodes = defs
            .into_iter()
            .map(|def| Node {
                fade: ChainFade {
                    frames: (def.transition.1 * f64::from(sample_rate)).round() as usize,
                    ..Default::default()
                },
                chain: def
                    .chain
                    .as_ref()
                    .map(|g| Voice::new(g.clone(), sample_rate)),
                def,
                voices: Vec::new(),
                admitted: 0,
                summed: blocks(MAX_CHANNELS),
                rate: 0.0,
                voice_nanos: 0,
                chain_silenced: false,
                chain_quiet: 0,
            })
            .collect();
        let lanes = vec![LaneCell::new(&bus_channels)];
        Ok(Mixer {
            profile: None,
            stems: None,
            audio_in: blocks(MAX_CHANNELS),
            buses: Buses::new(&bus_channels),
            nodes,
            order,
            groups,
            channels,
            bus_channels,
            bus_meters: Vec::new(),
            sample_rate,
            next_serial: 0,
            silenced: 0,
            max_voices: DEFAULT_MAX_VOICES,
            stolen: 0,
            muted: false,
            scratch: blocks(MAX_CHANNELS),
            processed: blocks(MAX_CHANNELS),
            lanes,
            fork: None,
        })
    }

    /// Report each node's cost to `profile`, whose nodes are numbered as this mixer's are.
    pub fn set_profile(&mut self, profile: Arc<Profile>) {
        self.profile = Some(profile);
    }

    pub fn set_bus_meters(&mut self, meters: Vec<(usize, Arc<Meter>)>) {
        self.bus_meters = meters;
    }

    /// Where the session's clock is at the start of the next block, for `clock.time`.
    pub fn set_node_instance(&mut self, index: usize, instance: u64) {
        if let Some(chain) = self.nodes.get_mut(index).and_then(|t| t.chain.as_mut()) {
            chain.set_instance(instance);
        }
    }

    pub fn set_clock(&mut self, clock: crate::Clock) {
        self.buses.clock = clock;
    }

    /// The audio input for the next block: `samples` is channel `channel` of it. A channel nobody sets
    /// is silent, and what is set stays until it is set again.
    pub fn set_audio_input(&mut self, channel: usize, samples: &[f32]) {
        if let Some(buffer) = self.audio_in.get_mut(channel) {
            let n = samples.len().min(block_size());
            buffer[..n].copy_from_slice(&samples[..n]);
            buffer[n..].fill(0.0);
        }
    }

    /// Let `fork` run chunks of voices on several threads. Call before the mixer renders in realtime:
    /// it makes room for them. The sound is the same with or without.
    pub fn set_fork(&mut self, fork: Option<Arc<dyn Fork>>) {
        let lanes = if fork.is_some() { LANES } else { 1 };
        while self.lanes.len() < lanes {
            self.lanes.push(LaneCell::new(&self.bus_channels));
        }
        self.lanes.truncate(lanes);
        self.fork = fork;
    }

    /// Reserve bounded voice storage before handing the mixer to rendering.
    pub fn prepare_realtime(&mut self, capacity: usize) {
        for node in &mut self.nodes {
            node.voices.reserve(capacity);
            if let Some(chain) = &mut node.chain {
                chain.set_realtime();
            }
            for (_, voice) in &mut node.voices {
                voice.set_realtime();
            }
        }
    }

    /// Immediate panic without destroying owned resources. A prepared clean
    /// mixer replaces this one, and the old mixer is freed in the background.
    pub fn silence(&mut self) {
        self.muted = true;
    }

    /// Start keeping what every node puts out (after its effects, before the master) and what every
    /// bus carries. Only for offline renders: it grows without bound.
    pub fn capture_stems(&mut self) {
        self.stems = Some(Stems {
            nodes: vec![Vec::new(); self.nodes.len()],
            buses: vec![Vec::new(); self.bus_channels.len()],
        });
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

    pub fn active_voices(&self) -> usize {
        self.nodes.iter().map(|t| t.voices.len()).sum()
    }

    /// How many voices and chains were silenced for producing NaN or infinity
    /// since this was last called.
    pub fn take_silenced(&mut self) -> usize {
        std::mem::take(&mut self.silenced)
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Update sounding voices' sends before transferring them to a mixer with
    /// a changed bus list. The graphs stay on their original definitions.
    pub fn remap_buses(&mut self, mapping: &[Option<usize>]) {
        self.remap_buses_with(&mut |i| mapping.get(i).copied().flatten());
    }

    pub fn remap_buses_with(&mut self, mapping: &mut impl FnMut(usize) -> Option<usize>) {
        for node in &mut self.nodes {
            for (_, voice) in &mut node.voices {
                voice.remap_buses_with(mapping);
            }
            for input in &mut node.def.inputs {
                if let ChainInput::Bus { bus, .. } = input
                    && let Some(next) = mapping(*bus)
                {
                    *bus = next;
                }
            }
            if let Some(previous) = &mut node.fade.previous {
                previous.remap_buses_with(mapping);
                for input in &mut node.fade.inputs {
                    if let ChainInput::Bus { bus, .. } = input
                        && let Some(next) = mapping(*bus)
                    {
                        *bus = next;
                    }
                }
            }
        }
    }

    /// Take over running state from `old`, which this mixer replaces: for each
    /// of this mixer's nodes, `transfers` says which of the old nodes' voices
    /// to continue with and whose effect chain (with its delay lines and filter
    /// memory) to keep. Voices keep playing on the instrument definition they
    /// started with. Old nodes that nothing claims are dropped.
    ///
    /// Chains carry memory at compatible nodes into the new definition.
    /// The caller must only carry state where bus identities are unchanged.
    pub fn adopt(&mut self, old: &mut Mixer, transfers: &[Transfer]) {
        assert_eq!(transfers.len(), self.nodes.len(), "one transfer per node");
        for (node, transfer) in self.nodes.iter_mut().zip(transfers) {
            if let Some(from) = transfer.voices_from
                && old.channels == self.channels
            {
                node.voices.append(&mut old.nodes[from].voices);
                if transfer.release {
                    for (_, voice) in &mut node.voices {
                        voice.release();
                    }
                }
            }
            if let Some(from) = transfer.chain_from {
                let previous = &mut old.nodes[from];
                let changed = node.def.transition.0 != previous.def.transition.0;
                if changed && node.def.reset_state {
                    // An explicit host reset keeps this fresh chain instead of transferring DSP memory.
                } else if changed && node.fade.frames > 0 {
                    // a different DSP: fade it in from the chain that was playing
                    node.fade.previous = previous.chain.take();
                    node.fade.inputs = std::mem::take(&mut previous.def.inputs);
                } else {
                    // the same DSP, or one that cannot crossfade: its memory comes straight across
                    if let (Some(chain), Some(old_chain)) = (&mut node.chain, &mut previous.chain) {
                        chain.carry_state(old_chain);
                    }
                    if !changed {
                        std::mem::swap(&mut node.fade, &mut previous.fade);
                    }
                }
            }
        }
        self.next_serial = self.next_serial.max(old.next_serial);
        self.silenced += std::mem::take(&mut old.silenced);
        // stems being kept carry on, if the new mixer has the same nodes and buses
        if let Some(stems) = old.stems.take_if(|s| {
            s.nodes.len() == self.nodes.len() && s.buses.len() == self.bus_channels.len()
        }) {
            self.stems = Some(stems);
        }
    }

    /// The channel count of each bus.
    pub fn bus_channels(&self) -> &[usize] {
        &self.bus_channels
    }
}
