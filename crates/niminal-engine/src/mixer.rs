//! Nodes, buses and the master: everything between voices and the speakers.
//!
//! Each block, buses are cleared; then nodes run in dependency order, so
//! everything that writes a bus runs before anything that reads it. That is
//! why a send reaches its reader in the same block, with no added latency.
//! The order is worked out from the graphs themselves, so the order nodes are
//! declared in never matters.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use crate::graph::Graph;
use crate::opcode::{block_size, blocks};
use crate::voice::{Voice, VoiceRule};

/// The most channels any signal path may carry (7.1.4 needs 12).
pub const MAX_CHANNELS: usize = 16;

/// Peaks shared with the control thread; rendering never allocates or locks.
#[derive(Debug, Default)]
pub struct Meter {
    peaks: [AtomicU32; MAX_CHANNELS],
    channels: AtomicU32,
    waveform: std::sync::OnceLock<crate::Waveform>,
}
impl Meter {
    pub fn observe(&self, blocks: &[Vec<f32>], frames: usize, sample: u64) {
        if let Some(waveform) = self.waveform.get() {
            waveform.observe(blocks, frames, sample);
        }
        self.channels.store(blocks.len() as u32, Ordering::Relaxed);
        for (peak, block) in self.peaks.iter().zip(blocks) {
            let value = block[..frames].iter().fold(0.0_f32, |a, b| a.max(b.abs()));
            peak.fetch_max(value.to_bits(), Ordering::Relaxed);
        }
    }
    pub fn waveform(&self) -> &crate::Waveform {
        self.waveform.get_or_init(crate::Waveform::default)
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
    data: Vec<Vec<Vec<f32>>>,
    /// What delayed sends put in over the last `BLOCK` samples, as a ring: a sample comes out
    /// where it went in, `BLOCK` samples later.
    late: Vec<Vec<Vec<f32>>>,
    /// What delayed sends have put in during the current block.
    staged: Vec<Vec<Vec<f32>>>,
    late_pos: usize,
    history: Vec<Vec<Vec<f32>>>,
    /// Where the session's clock is, for the block being mixed.
    pub clock: crate::Clock,
}

impl Buses {
    /// One bus per entry, with that many channels.
    pub fn new(channels: &[usize]) -> Self {
        Buses {
            data: channels.iter().map(|&n| blocks(n)).collect(),
            late: channels.iter().map(|&n| blocks(n)).collect(),
            staged: channels.iter().map(|&n| blocks(n)).collect(),
            late_pos: 0,
            history: channels.iter().map(|&n| blocks(n)).collect(),
            clock: crate::Clock::default(),
        }
    }

    /// Empty every bus, including what is on its way.
    pub fn clear(&mut self) {
        for bus in self
            .data
            .iter_mut()
            .chain(&mut self.late)
            .chain(&mut self.staged)
            .chain(&mut self.history)
        {
            bus.iter_mut().for_each(|c| c.fill(0.0));
        }
        self.late_pos = 0;
    }

    /// Start a block of `frames` samples: the buses hold what delayed sends put in a block ago
    /// (exactly `BLOCK` samples ago, whatever the length of the blocks in between).
    pub fn begin_block(&mut self, frames: usize) {
        let (first, rest) = self.ring_split(frames);
        let pos = self.late_pos;
        for ((data, late), staged) in self
            .data
            .iter_mut()
            .zip(&mut self.late)
            .zip(&mut self.staged)
        {
            for ((d, l), s) in data.iter_mut().zip(late.iter_mut()).zip(staged.iter_mut()) {
                s.fill(0.0);
                d.fill(0.0);
                d[..first].copy_from_slice(&l[pos..pos + first]);
                l[pos..pos + first].fill(0.0);
                d[first..frames].copy_from_slice(&l[..rest]);
                l[..rest].fill(0.0);
            }
        }
    }

    /// How `frames` samples written at `late_pos` of a ring one block long divide: this many up to
    /// the end of the ring, then this many from its start.
    fn ring_split(&self, frames: usize) -> (usize, usize) {
        let first = frames.min(block_size() - self.late_pos);
        (first, frames - first)
    }

    /// Finish the block: what delayed sends staged waits to arrive in `BLOCK` samples.
    pub fn end_block(&mut self, frames: usize) {
        let (first, rest) = self.ring_split(frames);
        let pos = self.late_pos;
        for (history, data) in self.history.iter_mut().zip(&self.data) {
            for (h, d) in history.iter_mut().zip(data) {
                h[pos..pos + first].copy_from_slice(&d[..first]);
                h[..rest].copy_from_slice(&d[first..frames]);
            }
        }
        for (late, staged) in self.late.iter_mut().zip(&self.staged) {
            for (l, s) in late.iter_mut().zip(staged) {
                for (l, s) in l[pos..pos + first].iter_mut().zip(&s[..first]) {
                    *l += s;
                }
                for (l, s) in l[..rest].iter_mut().zip(&s[first..frames]) {
                    *l += s;
                }
            }
        }
        self.late_pos = (self.late_pos + frames) % block_size();
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
    pub fn previous(&self, bus: usize, channel: usize, i: usize) -> f32 {
        self.history
            .get(bus)
            .and_then(|b| b.get(channel))
            .map_or(0.0, |c| c[(self.late_pos + i) % block_size()])
    }

    pub fn channels(&self, bus: usize) -> usize {
        self.data[bus].len()
    }
}

/// What a few voices send to the buses over one block, kept apart from the buses until it is added
/// to them in a fixed order, so the result does not depend on which voices ran when.
pub(crate) struct Sends {
    data: Vec<Vec<Vec<f32>>>,
    staged: Vec<Vec<Vec<f32>>>,
    /// Per bus: whether anything arrived this block, in `data` and in `staged`.
    touched: Vec<(bool, bool)>,
}

impl Sends {
    fn new(channels: &[usize]) -> Self {
        let shape = || channels.iter().map(|&n| blocks(n)).collect();
        Sends {
            data: shape(),
            staged: shape(),
            touched: vec![(false, false); channels.len()],
        }
    }

    pub(crate) fn add(&mut self, bus: usize, channel: usize, samples: &[f32]) {
        if let Some(c) = self.data.get_mut(bus).and_then(|b| b.get_mut(channel)) {
            self.touched[bus].0 = true;
            c.iter_mut().zip(samples).for_each(|(d, s)| *d += s);
        }
    }

    pub(crate) fn add_delayed(&mut self, bus: usize, channel: usize, samples: &[f32]) {
        if let Some(c) = self.staged.get_mut(bus).and_then(|b| b.get_mut(channel)) {
            self.touched[bus].1 = true;
            c.iter_mut().zip(samples).for_each(|(d, s)| *d += s);
        }
    }

    /// Add everything to `buses` and start over.
    fn drain_into(&mut self, buses: &mut Buses, frames: usize) {
        for (bus, touched) in self.touched.iter_mut().enumerate() {
            let (now, later) = std::mem::take(touched);
            for (c, (data, staged)) in self.data[bus]
                .iter_mut()
                .zip(&mut self.staged[bus])
                .enumerate()
            {
                if now {
                    buses.add(bus, c, &data[..frames]);
                    data.fill(0.0);
                }
                if later {
                    buses.add_delayed(bus, c, &staged[..frames]);
                    staged.fill(0.0);
                }
            }
        }
    }
}

/// Where a node's output goes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    #[default]
    Master,
    Bus(usize),
}

/// What feeds one of a node chain's external (mono) inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainInput {
    /// One channel of the summed output of the node's voices.
    It { channel: usize },
    /// One channel of a bus.
    Bus { bus: usize, channel: usize },
    /// One channel of the audio input (a microphone, a line in, a file being rendered).
    Audio { channel: usize },
}

#[derive(Clone, Default)]
pub struct NodeDef {
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
    /// Buses that this node's voices may send to.
    pub voice_sends: Vec<usize>,
    /// Shared buffers that this node's voices may write.
    pub voice_buffer_writes: Vec<usize>,
    /// Shared buffers that this node's voices may read.
    pub voice_buffer_reads: Vec<usize>,
}

impl NodeDef {
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

/// For each node, the nodes that must run after it: those that read a bus or buffer it writes.
fn dependencies(nodes: &[NodeDef]) -> Vec<Vec<usize>> {
    let n = nodes.len();
    let reads: Vec<_> = nodes.iter().map(NodeDef::reads).collect();
    let writes: Vec<_> = nodes.iter().map(NodeDef::writes).collect();
    let buf_reads: Vec<_> = nodes.iter().map(NodeDef::buffer_reads).collect();
    let buf_writes: Vec<_> = nodes.iter().map(NodeDef::buffer_writes).collect();
    (0..n)
        .map(|w| {
            (0..n)
                .filter(|&r| {
                    writes[w].iter().any(|b| reads[r].contains(b))
                        || (w != r && buf_writes[w].iter().any(|b| buf_reads[r].contains(b)))
                })
                .collect()
        })
        .collect()
}

/// `order` cut into the longest runs of nodes in which none depends on an earlier one.
fn independent_runs(nodes: &[NodeDef], order: &[usize]) -> Vec<std::ops::Range<usize>> {
    let edges = dependencies(nodes);
    let mut runs = Vec::new();
    let mut start = 0;
    for k in 0..order.len() {
        if order[start..k]
            .iter()
            .any(|&w| edges[w].contains(&order[k]))
        {
            runs.push(start..k);
            start = k;
        }
    }
    if start < order.len() {
        runs.push(start..order.len());
    }
    runs
}

/// An order in which to run nodes so that every bus and shared buffer is
/// written before it is read. A cycle comes back as the nodes that form it, in
/// order; a node that both writes and reads one bus is a cycle of one. A node
/// that both writes and reads the same buffer does not count as a cycle (the
/// write and read order inside its graph already decides that).
pub fn execution_order(nodes: &[NodeDef]) -> Result<Vec<usize>, Vec<usize>> {
    let n = nodes.len();
    let edges = dependencies(nodes);
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

    // Everything left is on or downstream of a cycle. Each such node is still
    // waiting on another unfinished one, so walking backwards must repeat a
    // node, and the repeated stretch is the cycle.
    let stuck: Vec<usize> = (0..n).filter(|i| !order.contains(i)).collect();
    let mut path = vec![stuck[0]];
    loop {
        let here = *path.last().unwrap();
        let before = stuck
            .iter()
            .copied()
            .find(|&w| edges[w].contains(&here))
            .unwrap();
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
    fn observe(&self, voices: usize, voice_nanos: u64, chain_nanos: u64) {
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

/// Shares jobs out among threads.
pub trait Fork: Send + Sync {
    /// Runs `jobs` jobs, numbered from 0, each exactly once, perhaps on several threads, and returns
    /// when all are done. The mixer's output never depends on which thread ran which job.
    fn run(&self, jobs: usize, job: &(dyn Fn(usize) + Sync));

    /// Whether `jobs` jobs that take `work_ns` nanoseconds one after another are better shared out
    /// than run in turn. It only ever affects how fast the sound is made, never what it is.
    fn worth_it(&self, work_ns: u64, jobs: usize) -> bool;
}

/// What one chunk of voices puts out and sends while it runs.
struct Lane {
    summed: Vec<Vec<f32>>,
    scratch: Vec<Vec<f32>>,
    sends: Sends,
    /// How long the last job on this lane took, if timed.
    nanos: u64,
}

impl Lane {
    fn new(bus_channels: &[usize]) -> Self {
        Lane {
            summed: blocks(MAX_CHANNELS),
            scratch: blocks(MAX_CHANNELS),
            sends: Sends::new(bus_channels),
            nanos: 0,
        }
    }
}

/// Chunks of voices that can run at once.
const LANES: usize = 16;

/// Some of a node's voices, which one job runs: voices `ptr[..len]`.
#[derive(Clone, Copy)]
struct ChunkRef {
    node: usize,
    ptr: *mut (u64, Voice),
    len: usize,
}

// SAFETY: the chunks of a wave are different voices (of different nodes, or different stretches of
// one node's), and nothing else touches a node's voices until the wave is done.
unsafe impl Send for ChunkRef {}
unsafe impl Sync for ChunkRef {}

const NO_CHUNK: ChunkRef = ChunkRef {
    node: 0,
    ptr: std::ptr::null_mut(),
    len: 0,
};

/// A lane that one job at a time uses.
struct LaneCell(std::cell::UnsafeCell<Lane>);

// SAFETY: while jobs run, job `i` is the only one to touch lane `i` (`Fork::run` runs each job once),
// and the mixer touches lanes only before and after.
unsafe impl Sync for LaneCell {}

impl LaneCell {
    fn new(bus_channels: &[usize]) -> Self {
        LaneCell(std::cell::UnsafeCell::new(Lane::new(bus_channels)))
    }
}

/// How many voices are summed together before joining the rest. Fixed, so that the order in which
/// floating-point sums are made depends only on the notes played and never on how they are scheduled.
pub const VOICE_CHUNK: usize = 8;

/// How long a node's effect chain must have had nothing going in and nothing coming out before it is
/// left alone until something arrives, in seconds.
const CHAIN_REST_SECONDS: f32 = 0.1;

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

    /// How long a sounding note is scheduled to last; see [`Voice::set_dur`].
    pub fn silence_owner(&mut self, owner: u64) {
        for node in &mut self.nodes {
            for (_, voice) in &mut node.voices {
                if voice.owner() == owner {
                    voice.set_activity_sample(self.buses.clock.sample);
                    voice.silence();
                }
            }
        }
    }
    pub fn attach_voice_activity(
        &mut self,
        id: VoiceId,
        activity: Arc<crate::Activity>,
        sample: u64,
    ) {
        activity.start(sample, id.serial);
        if let Some(node) = self.nodes.get_mut(id.node)
            && let Some((_, voice)) = node
                .voices
                .iter_mut()
                .find(|(s, v)| *s == id.serial || v.holds(id.serial))
        {
            voice.attach_activity(activity);
        }
    }
    pub fn finish_activities(&self, sample: u64, reason: u32) {
        for node in &self.nodes {
            for (_, voice) in &node.voices {
                voice.finish_activities(sample, reason);
            }
        }
    }
    pub fn set_voice_owner(&mut self, id: VoiceId, owner: u64) {
        if let Some(node) = self.nodes.get_mut(id.node)
            && let Some((_, voice)) = node.voices.iter_mut().find(|(s, _)| *s == id.serial)
        {
            voice.set_owner(owner);
        }
    }
    pub fn set_voice_dur(&mut self, id: VoiceId, seconds: f32) {
        if let Some(node) = self.nodes.get_mut(id.node)
            && let Some((_, voice)) = node
                .voices
                .iter_mut()
                .find(|(serial, _)| *serial == id.serial)
        {
            voice.set_dur(seconds);
        }
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

    /// Admit an already constructed voice without allocating. Capacity is a
    /// hard limit; rejected voices remain owned by the caller for retirement.
    // the voice comes back in the error so that its owner can retire it without allocating
    #[allow(clippy::result_large_err)]
    pub fn note_prepared(&mut self, node: usize, voice: Voice) -> Result<VoiceId, Voice> {
        if self.muted
            || node >= self.nodes.len()
            || voice.channels() != self.channels
            || self.nodes[node].voices.len() == self.nodes[node].voices.capacity()
        {
            return Err(voice);
        }
        if let Some(group) = voice.group() {
            for (_, other) in self.nodes.iter_mut().flat_map(|t| t.voices.iter_mut()) {
                if other.group() == Some(group) {
                    other.choke();
                }
            }
        }
        let mut voice = voice;
        if let Some(rule) = voice.rule() {
            self.glide_into(node, &rule, &mut voice);
            self.make_room(node, &rule);
        }
        self.steal_if_full();
        let serial = self.next_serial;
        self.next_serial += 1;
        voice.start_activities(self.buses.clock.sample, serial);
        voice.start_note();
        if voice.rule().is_some_and(|r| r.legato) {
            voice.hold(serial);
        }
        self.nodes[node].admit(serial, voice);
        Ok(VoiceId { node, serial })
    }

    /// Immediate panic without destroying owned resources. A prepared clean
    /// mixer replaces this one, and the old mixer is freed in the background.
    pub fn silence(&mut self) {
        self.muted = true;
    }

    /// Start a note on `node`, setting `(parameter index, value)` pairs. The
    /// graph must produce as many channels as the mixer's master has.
    ///
    /// A note in a choke `group` ends every sounding note in the same group
    /// (on any node), with a short fade.
    pub fn note_on(
        &mut self,
        node: usize,
        graph: Arc<Graph>,
        params: &[(usize, f32)],
        group: Option<u64>,
    ) -> VoiceId {
        self.note_on_with(node, graph, params, group, None)
    }

    /// Like [`Mixer::note_on`], for an instrument that limits its own voices.
    pub fn note_on_with(
        &mut self,
        node: usize,
        graph: Arc<Graph>,
        params: &[(usize, f32)],
        group: Option<u64>,
        rule: Option<VoiceRule>,
    ) -> VoiceId {
        assert_eq!(
            graph.channels(),
            self.channels,
            "an instrument must output the master's channel count"
        );
        if group.is_some() {
            for (_, v) in self.nodes.iter_mut().flat_map(|t| t.voices.iter_mut()) {
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
            if let Some(id) = self.merge_legato(node, &voice) {
                return id;
            }
            self.glide_into(node, rule, &mut voice);
            self.make_room(node, rule);
        }
        self.steal_if_full();
        let serial = self.next_serial;
        self.next_serial += 1;
        voice.start_activities(self.buses.clock.sample, serial);
        voice.start_note();
        if rule.is_some_and(|r| r.legato) {
            voice.hold(serial);
        }
        self.nodes[node].admit(serial, voice);
        VoiceId { node, serial }
    }

    /// A legato note that starts while its instrument's voice is held takes that voice over, so the
    /// envelopes carry on and only the pitch moves. The `note` it came from is only read, so a caller
    /// that must not free memory can retire it afterwards. Allocates nothing.
    pub fn merge_legato(&mut self, node: usize, note: &Voice) -> Option<VoiceId> {
        let rule = note.rule().filter(|r| r.legato && !self.muted)?;
        let serial = self.next_serial;
        let (_, voice) = self.nodes.get_mut(node)?.voices.iter_mut().find(|(_, v)| {
            v.is_legato_open() && v.rule().is_some_and(|r| r.instrument == rule.instrument)
        })?;
        voice.legato_on(serial, note);
        self.next_serial += 1;
        Some(VoiceId { node, serial })
    }

    /// A glissando into `voice`: it starts at the pitch the instrument's newest note had reached.
    fn glide_into(&self, node: usize, rule: &VoiceRule, voice: &mut Voice) {
        if rule.glide <= 0.0 {
            return;
        }
        let Some(node) = self.nodes.get(node) else {
            return;
        };
        let previous = node
            .voices
            .iter()
            .filter(|(_, v)| {
                !v.is_choked() && v.rule().is_some_and(|r| r.instrument == rule.instrument)
            })
            .max_by_key(|(serial, _)| *serial);
        if let Some((_, v)) = previous {
            voice.glide_from(v.pitch_now(), self.sample_rate);
        }
    }

    /// Make room for a note under `rule`: release the sounding note of the same pitch if asked, and
    /// end the oldest notes of the instrument until fewer than `max` are left. Allocates nothing.
    fn make_room(&mut self, node: usize, rule: &VoiceRule) {
        let Some(node) = self.nodes.get_mut(node) else {
            return;
        };
        let mine = |v: &Voice| v.rule().is_some_and(|r| r.instrument == rule.instrument);
        if rule.release_same_pitch {
            for (_, v) in node.voices.iter_mut() {
                if mine(v) && !v.is_released() && v.rule().is_some_and(|r| r.pitch == rule.pitch) {
                    v.release();
                }
            }
        }
        loop {
            // the voice to end: the quietest (the oldest of equals), or just the oldest
            let (mut count, mut ends) = (0usize, None::<(u64, f32)>);
            for (serial, v) in node.voices.iter() {
                if mine(v) && !v.is_choked() {
                    count += 1;
                    let level = if rule.steal_quietest { v.level() } else { 0.0 };
                    if ends.is_none_or(|(s, l)| level < l || (level == l && *serial < s)) {
                        ends = Some((*serial, level));
                    }
                }
            }
            let Some((ends, _)) = ends.filter(|_| count >= rule.max.max(1)) else {
                break;
            };
            if let Some((_, v)) = node.voices.iter_mut().find(|(serial, _)| *serial == ends) {
                v.choke();
            }
        }
    }

    pub fn set_control(&mut self, key: &str, start: f32, target: f32, frames: u64, elapsed: u64) {
        for node in &mut self.nodes {
            for (_, voice) in &mut node.voices {
                voice.set_control(key, start, target, frames, elapsed);
            }
            if let Some(chain) = &mut node.chain {
                let local = node
                    .def
                    .controls
                    .iter()
                    .find(|(public, _)| public == key)
                    .map_or(key, |(_, local)| local.as_str());
                // A template's private controls are changed through its instance keys only.
                if !node
                    .def
                    .controls
                    .iter()
                    .any(|(_, template)| template == key)
                    || node.def.controls.iter().any(|(public, _)| public == key)
                {
                    node.chain_quiet = 0;
                    chain.set_control(local, start, target, frames, elapsed);
                }
            }
        }
    }

    /// Place a sounding note among the speakers; see [`Voice::set_pan`].
    pub fn set_voice_pan(&mut self, id: VoiceId, gains: Option<[f32; MAX_CHANNELS]>) {
        if let Some(node) = self.nodes.get_mut(id.node)
            && let Some((_, voice)) = node
                .voices
                .iter_mut()
                .find(|(serial, _)| *serial == id.serial)
        {
            voice.set_pan(gains);
        }
    }

    pub fn set_voice_control(
        &mut self,
        id: VoiceId,
        key: &str,
        start: f32,
        target: f32,
        frames: u64,
        elapsed: u64,
    ) {
        if let Some(node) = self.nodes.get_mut(id.node)
            && let Some((_, voice)) = node
                .voices
                .iter_mut()
                .find(|(serial, _)| *serial == id.serial)
        {
            voice.set_control(key, start, target, frames, elapsed);
        }
    }

    /// Release a note. A voice that has already finished is ignored. Voices keep
    /// their id when they move between nodes (see [`Mixer::adopt`]), so the
    /// node it started on is only a hint.
    pub fn release(&mut self, id: VoiceId) {
        let ends = |(s, v): &(u64, Voice)| *s == id.serial || v.holds(id.serial);
        let sample = self.buses.clock.sample;
        let hinted = self
            .nodes
            .get_mut(id.node)
            .and_then(|t| t.voices.iter_mut().find(|n| ends(n)));
        if let Some((_, v)) = hinted {
            v.set_activity_sample(sample);
            v.note_off(id.serial);
            return;
        }
        for node in &mut self.nodes {
            if let Some((_, v)) = node.voices.iter_mut().find(|n| ends(n)) {
                v.set_activity_sample(sample);
                v.note_off(id.serial);
                return;
            }
        }
    }

    /// Set a parameter of a sounding note by name, as a keyboard's pitch bend does. A note that has
    /// ended, or an instrument without the parameter, ignores it. Allocates nothing.
    pub fn set_voice_param(&mut self, id: VoiceId, name: &str, value: f32) {
        let ends = |(s, v): &(u64, Voice)| *s == id.serial || v.holds(id.serial);
        for node in &mut self.nodes {
            if let Some((_, v)) = node.voices.iter_mut().find(|n| ends(n)) {
                v.set_param_by_name(name, value);
                return;
            }
        }
    }

    /// Move a parameter of a sounding note, by its index, to `value` over `frames` (at once if 0). A note
    /// that has ended is ignored. Allocates nothing.
    pub fn set_voice_param_index(&mut self, id: VoiceId, index: usize, value: f32, frames: u64) {
        let ends = |(s, v): &(u64, Voice)| *s == id.serial || v.holds(id.serial);
        for node in &mut self.nodes {
            if let Some((_, v)) = node.voices.iter_mut().find(|n| ends(n)) {
                v.set_param_glide(index, value, frames);
                return;
            }
        }
    }

    pub fn swap_voice_buffers(&mut self, id: VoiceId, key: u64, banks: &mut [crate::BufferBank]) {
        for node in &mut self.nodes {
            if let Some((_, voice)) = node
                .voices
                .iter_mut()
                .find(|(s, v)| *s == id.serial || v.holds(id.serial))
            {
                voice.swap_buffers(key, banks);
                return;
            }
        }
    }
    pub fn swap_chain_buffers(&mut self, node: usize, key: u64, banks: &mut [crate::BufferBank]) {
        if let Some(chain) = self.nodes.get_mut(node).and_then(|t| t.chain.as_mut()) {
            chain.swap_buffers(key, banks);
        }
    }
    pub fn chain_param_value(&self, node: usize, param: &str) -> Option<f32> {
        self.nodes.get(node)?.chain.as_ref()?.param_value(param)
    }
    pub fn swap_chain_mapping(
        &mut self,
        node: usize,
        param: &str,
        mapping: &mut Option<Box<crate::ParamMapping>>,
    ) {
        if let Some(chain) = self.nodes.get_mut(node).and_then(|t| t.chain.as_mut()) {
            chain.swap_mapping(param, mapping);
        }
    }
    pub fn swap_voice_mapping(
        &mut self,
        id: VoiceId,
        name: &str,
        mapping: &mut Option<Box<crate::ParamMapping>>,
    ) {
        let ends = |(s, v): &(u64, Voice)| *s == id.serial || v.holds(id.serial);
        for node in &mut self.nodes {
            if let Some((_, voice)) = node.voices.iter_mut().find(|v| ends(v)) {
                voice.swap_mapping(name, mapping);
                return;
            }
        }
    }

    pub fn set_voice_param_glide(&mut self, id: VoiceId, name: &str, value: f32, frames: u64) {
        let ends = |(s, v): &(u64, Voice)| *s == id.serial || v.holds(id.serial);
        for node in &mut self.nodes {
            if let Some((_, voice)) = node.voices.iter_mut().find(|v| ends(v)) {
                if let Some(index) = voice.param_index(name) {
                    voice.set_param_glide(index, value, frames);
                }
                return;
            }
        }
    }

    /// Silence everything at once: stop every voice, clear every effect's
    /// memory (delay lines, reverb tails) and empty the buses.
    pub fn panic(&mut self) {
        let rate = self.sample_rate;
        for node in &mut self.nodes {
            node.voices.clear();
            node.fade = ChainFade::default();
            node.chain = node.def.chain.as_ref().map(|g| Voice::new(g.clone(), rate));
            node.chain_silenced = false;
            node.chain_quiet = 0;
        }
        self.buses.clear();
        self.muted = false;
    }

    /// Start keeping what every node puts out (after its effects, before the master) and what every
    /// bus carries. Only for offline renders: it grows without bound.
    pub fn capture_stems(&mut self) {
        self.stems = Some(Stems {
            nodes: vec![Vec::new(); self.nodes.len()],
            buses: vec![Vec::new(); self.bus_channels.len()],
        });
    }

    /// Fill the captured stems with silence up to `len` samples each, for stretches the caller skipped
    /// mixing because nothing could be heard.
    pub fn pad_stems(&mut self, len: usize) {
        if let Some(stems) = &mut self.stems {
            for channel in stems
                .nodes
                .iter_mut()
                .chain(stems.buses.iter_mut())
                .flatten()
            {
                if channel.len() < len {
                    channel.resize(len, 0.0);
                }
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
            .nodes
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

    /// Mix `frames` samples (at most [`BLOCK`]) of everything into `out`, one
    /// block per master channel.
    pub fn process(&mut self, out: &mut [Vec<f32>], frames: usize) {
        self.process_inner(out, frames, None);
    }

    /// Render while retaining finished voices for off-thread destruction.
    /// `retired` must have been reserved by the caller before rendering.
    pub fn process_retained(
        &mut self,
        out: &mut [Vec<f32>],
        frames: usize,
        retired: &mut Vec<Voice>,
    ) {
        self.process_inner(out, frames, Some(retired));
    }

    /// Make the voices of the nodes `order[group]` sound, leaving each node's sum in its `summed`.
    /// Chunks of voices run side by side in waves, one lane each, and their sums and sends are added
    /// in node and chunk order afterwards: the output is the same however many threads help.
    fn render_voices(&mut self, group: std::ops::Range<usize>, frames: usize) {
        let n = self.channels;
        for k in group.clone() {
            let t = self.order[k];
            self.nodes[t].summed[..n]
                .iter_mut()
                .for_each(|c| c[..frames].fill(0.0));
            self.nodes[t].voice_nanos = 0;
        }
        let width = self.lanes.len();
        let mut refs = [NO_CHUNK; LANES];
        let mut count = 0;
        for k in group {
            let t = self.order[k];
            // voices that write a shared buffer must not run side by side: who writes last would be luck
            let alone = !self.nodes[t].def.voice_buffer_writes.is_empty();
            if alone && count > 0 {
                self.run_wave(&refs[..count], frames, true);
                count = 0;
            }
            let (len, mut start) = (self.nodes[t].voices.len(), 0);
            while start < len {
                let voices = &mut self.nodes[t].voices;
                let key = voices[start].1.chunk();
                let take = voices[start..]
                    .iter()
                    .position(|(_, v)| v.chunk() != key)
                    .unwrap_or(len - start);
                refs[count] = ChunkRef {
                    node: t,
                    ptr: voices[start..].as_mut_ptr(),
                    len: take,
                };
                count += 1;
                start += take;
                if count == width || alone {
                    self.run_wave(&refs[..count], frames, !alone);
                    count = 0;
                }
            }
        }
        if count > 0 {
            self.run_wave(&refs[..count], frames, true);
        }
    }

    /// Run chunks of voices, one lane each, and add what they made to their nodes' sums and the buses,
    /// in order. With `share` they may be shared out among threads, if that is worth it.
    fn run_wave(&mut self, refs: &[ChunkRef], frames: usize, share: bool) {
        let n = self.channels;
        let Mixer {
            fork,
            lanes,
            buses,
            nodes,
            ..
        } = self;
        let timer = fork.as_deref();
        let shared = timer.filter(|fork| {
            let work: f32 = refs
                .iter()
                .map(|r| r.len as f32 * frames as f32 * nodes[r.node].rate)
                .sum();
            share && refs.len() > 1 && fork.worth_it(work as u64, refs.len())
        });
        {
            let (lanes, buses) = (&*lanes, &*buses);
            let job = |i: usize| {
                // each thread has its own idea of how to treat tiny numbers
                let _flush = crate::denormals::FlushToZero::new();
                let started = std::time::Instant::now();
                // SAFETY: see `LaneCell`
                let Lane {
                    summed,
                    scratch,
                    sends,
                    nanos,
                } = unsafe { &mut *lanes[i].0.get() };
                summed[..n].iter_mut().for_each(|c| c[..frames].fill(0.0));
                let r = refs[i];
                // SAFETY: see `ChunkRef`
                let chunk = unsafe { std::slice::from_raw_parts_mut(r.ptr, r.len) };
                for (_, voice) in chunk {
                    voice.process_sending(&mut scratch[..n], frames, buses, sends);
                    for (sum, voice_out) in summed[..n].iter_mut().zip(&scratch[..n]) {
                        for (s, v) in sum[..frames].iter_mut().zip(&voice_out[..frames]) {
                            *s += v;
                        }
                    }
                }
                *nanos = started.elapsed().as_nanos() as u64;
            };
            match shared {
                Some(fork) => fork.run(refs.len(), &job),
                None => (0..refs.len()).for_each(job),
            }
        }
        for (i, r) in refs.iter().enumerate() {
            let Lane {
                summed: lane_sum,
                sends,
                nanos,
                ..
            } = lanes[i].0.get_mut();
            let node = &mut nodes[r.node];
            for (sum, chunk) in node.summed[..n].iter_mut().zip(&lane_sum[..n]) {
                for (s, v) in sum[..frames].iter_mut().zip(&chunk[..frames]) {
                    *s += v;
                }
            }
            sends.drain_into(buses, frames);
            node.voice_nanos += *nanos;
            // short blocks are mostly fixed costs, which say little about a voice
            if timer.is_some() && frames >= block_size() / 2 {
                let sample = *nanos as f32 / (r.len * frames) as f32;
                node.rate = if node.rate == 0.0 {
                    sample
                } else {
                    node.rate + (sample - node.rate) / 8.0
                };
            }
        }
    }

    fn process_inner(
        &mut self,
        out: &mut [Vec<f32>],
        frames: usize,
        mut retired: Option<&mut Vec<Voice>>,
    ) {
        assert!(
            frames <= block_size(),
            "block too long: {frames} > {}",
            block_size()
        );
        assert_eq!(
            out.len(),
            self.channels,
            "one output block per master channel"
        );
        // every way of rendering audio comes through here, so they all treat tiny numbers alike
        let _flush = crate::denormals::FlushToZero::new();
        out.iter_mut().for_each(|c| c[..frames].fill(0.0));
        if self.muted {
            self.buses.clear();
            return;
        }
        self.buses.begin_block(frames);

        let n = self.channels;
        let rest_after = (CHAIN_REST_SECONDS * self.sample_rate) as usize;

        for g in 0..self.groups.len() {
            let group = self.groups[g].clone();
            // the voices of every node in the run first, side by side, then each node's chain in turn
            self.render_voices(group.clone(), frames);
            for k in group {
                let t = self.order[k];
                let (scratch, processed) = (&mut self.scratch, &mut self.processed);
                let node = &mut self.nodes[t];
                if node.fade.elapsed >= node.fade.frames && node.fade.previous.is_some() {
                    if let Some(retired) = retired.as_deref_mut() {
                        if retired.len() < retired.capacity() {
                            retired.push(node.fade.previous.take().unwrap());
                        }
                    } else {
                        node.fade.previous = None;
                    }
                }

                if let Some(retired) = retired.as_deref_mut() {
                    let mut i = 0;
                    while i < node.voices.len() {
                        if node.voices[i].1.is_finished() && retired.len() < retired.capacity() {
                            let (_, voice) = node.voices.remove(i);
                            voice.finish_activities(
                                self.buses.clock.sample + frames as u64,
                                voice.end_reason(),
                            );
                            self.silenced += usize::from(voice.is_poisoned());
                            retired.push(voice);
                        } else {
                            i += 1;
                        }
                    }
                } else {
                    self.silenced += node.voices.iter().filter(|(_, v)| v.is_poisoned()).count();
                    node.voices.retain(|(_, v)| {
                        if v.is_finished() {
                            v.finish_activities(
                                self.buses.clock.sample + frames as u64,
                                v.end_reason(),
                            );
                            false
                        } else {
                            true
                        }
                    });
                }

                let chain_started = std::time::Instant::now();
                let result: &[Vec<f32>] = match &mut node.chain {
                    None => &node.summed[..n],
                    Some(chain) => {
                        let out_channels = chain.channels();
                        let heard = |input: &ChainInput| -> &[f32] {
                            match *input {
                                ChainInput::It { channel } => &node.summed[channel][..frames],
                                ChainInput::Audio { channel } => &self.audio_in[channel][..frames],
                                ChainInput::Bus { bus, channel } => {
                                    self.buses.get(bus, channel, frames)
                                }
                            }
                        };
                        let silent_in = node
                            .def
                            .inputs
                            .iter()
                            .all(|input| heard(input).iter().all(|s| *s == 0.0));
                        if silent_in
                            && node.chain_quiet >= rest_after
                            && !(node.fade.previous.is_some()
                                && node.fade.elapsed < node.fade.frames)
                        {
                            // nothing to ring out and nothing coming in: the chain stays as it is, and says nothing
                            processed[..out_channels]
                                .iter_mut()
                                .for_each(|c| c[..frames].fill(0.0));
                        } else {
                            for (i, input) in node.def.inputs.iter().enumerate() {
                                chain.set_input(i, heard(input));
                            }
                            if let Some(previous) = &mut node.fade.previous {
                                for (i, input) in node.fade.inputs.iter().enumerate() {
                                    previous.set_input(i, heard(input));
                                }
                            }
                            chain.process_blocks(
                                &mut processed[..out_channels],
                                frames,
                                &mut self.buses,
                            );
                            if node.fade.elapsed < node.fade.frames
                                && let Some(previous) = &mut node.fade.previous
                            {
                                previous.process_blocks(
                                    &mut scratch[..out_channels],
                                    frames,
                                    &mut self.buses,
                                );
                                for (next, old) in processed[..out_channels]
                                    .iter_mut()
                                    .zip(&scratch[..out_channels])
                                {
                                    for i in 0..frames {
                                        let mix = ((node.fade.elapsed + i) as f32
                                            / node.fade.frames as f32)
                                            .min(1.0);
                                        next[i] = old[i] * (1.0 - mix) + next[i] * mix;
                                    }
                                }
                                node.fade.elapsed = node.fade.elapsed.saturating_add(frames);
                            }
                            if chain.is_poisoned() && !node.chain_silenced {
                                node.chain_silenced = true;
                                self.silenced += 1;
                            }
                            node.chain_quiet = if silent_in && chain.is_quiet(frames) {
                                node.chain_quiet + frames
                            } else {
                                0
                            };
                        }
                        &processed[..out_channels]
                    }
                };

                if let Some(load) = self.profile.as_ref().and_then(|p| p.nodes.get(t))
                    && frames >= block_size() / 2
                {
                    let chain_nanos = if node.chain.is_some() {
                        chain_started.elapsed().as_nanos() as u64
                    } else {
                        0
                    };
                    load.observe(node.voices.len(), node.voice_nanos, chain_nanos);
                }
                if let Some(meter) = &node.def.meter {
                    meter.observe(result, frames, self.buses.clock.sample);
                }
                if let Some(stems) = &mut self.stems {
                    keep(&mut stems.nodes[t], result, frames);
                }
                match node.def.route {
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
        for (bus, meter) in &self.bus_meters {
            meter.observe(&self.buses.data[*bus], frames, self.buses.clock.sample);
        }
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
                if changed && node.fade.frames > 0 {
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

/// Append the first `frames` samples of every channel of `block` to `kept`.
fn keep(kept: &mut Vec<Vec<f32>>, block: &[Vec<f32>], frames: usize) {
    kept.resize(block.len(), Vec::new());
    for (channel, samples) in kept.iter_mut().zip(block) {
        channel.extend_from_slice(&samples[..frames]);
    }
}
