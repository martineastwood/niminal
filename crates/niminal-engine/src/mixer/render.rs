//! Block rendering, parallel voice waves, and effect-chain mixing.

use crate::opcode::{block_size, blocks};
use crate::voice::Voice;

use super::{ChainInput, MAX_CHANNELS, Mixer, Route, Sends};

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
pub(super) const LANES: usize = 16;

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
// SAFETY: as for `Send` above.
unsafe impl Sync for ChunkRef {}

const NO_CHUNK: ChunkRef = ChunkRef {
    node: 0,
    ptr: std::ptr::null_mut(),
    len: 0,
};

/// A lane that one job at a time uses.
pub(super) struct LaneCell(std::cell::UnsafeCell<Lane>);

// SAFETY: while jobs run, job `i` is the only one to touch lane `i` (`Fork::run` runs each job once),
// and the mixer touches lanes only before and after.
unsafe impl Sync for LaneCell {}

impl LaneCell {
    pub(super) fn new(bus_channels: &[usize]) -> Self {
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
                            retired.push(node.fade.previous.take().expect("checked is_some above"));
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
                        // Input-free chains are generators: silence can be part of their rhythm.
                        if !node.def.inputs.is_empty()
                            && silent_in
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
}

/// Append the first `frames` samples of every channel of `block` to `kept`.
fn keep(kept: &mut Vec<Vec<f32>>, block: &[Vec<f32>], frames: usize) {
    kept.resize(block.len(), Vec::new());
    for (channel, samples) in kept.iter_mut().zip(block) {
        channel.extend_from_slice(&samples[..frames]);
    }
}
