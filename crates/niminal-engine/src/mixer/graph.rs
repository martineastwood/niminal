//! Node routing definitions and topological execution ordering.

use std::sync::Arc;

use crate::graph::Graph;

use super::Meter;

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
    /// Clear persistent DSP state when the replacement generation changes. Voices and controls remain.
    pub reset_state: bool,
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
pub(super) fn independent_runs(nodes: &[NodeDef], order: &[usize]) -> Vec<std::ops::Range<usize>> {
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
        let here = *path.last().expect("the path starts with one node");
        let before = stuck
            .iter()
            .copied()
            .find(|&w| edges[w].contains(&here))
            .expect("every stuck node is waiting on another stuck node");
        if let Some(at) = path.iter().position(|&p| p == before) {
            let mut cycle = path[at..].to_vec();
            cycle.reverse();
            return Err(cycle);
        }
        path.push(before);
    }
}
