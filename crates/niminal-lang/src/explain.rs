//! `niminal explain`: where a compiled graph spends its time, by rate.

use std::collections::BTreeMap;
use std::fmt::Write;

use niminal_engine::{Graph, KernelStats};

/// An oscillator at or below this many hz that only sets a parameter is an LFO.
const LFO_HZ: f32 = 30.0;

/// `explain` works at the rate `niminal server` and `niminal-live render` default to.
const SAMPLE_RATE: f32 = 48_000.0;

use crate::program::Program;

impl Program {
    /// A plain-text report of what each instrument and node chain runs every sample, and what runs
    /// only once per block, so a `control { ... }` scope can be placed where it saves work.
    pub fn explain(&self) -> String {
        let mut out = String::new();
        for instrument in &self.instruments {
            report(&mut out, &instrument.name, &instrument.graph);
        }
        for node in self.nodes.iter().filter(|n| !n.name.is_empty()) {
            if let Some(chain) = &node.def.chain {
                report(&mut out, &format!("{} (chain)", node.name), chain);
            }
        }
        if out.is_empty() {
            out.push_str("nothing to explain: no instruments or node chains\n");
        }
        out
    }
}

/// How many times each name occurs, as `osc, lpf ×2`.
fn tally(names: &[String]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for name in names {
        *counts.entry(name).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(name, n)| {
            if n == 1 {
                name.to_string()
            } else {
                format!("{name} ×{n}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn report(out: &mut String, name: &str, graph: &Graph) {
    let mut sample_ops: Vec<String> = Vec::new();
    let mut block_ops: Vec<String> = Vec::new();
    let mut kernels: Vec<(String, KernelStats)> = Vec::new();
    let mut suggestions: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for node in graph.describe(SAMPLE_RATE) {
        for warning in &node.warnings {
            warnings.push(warning.clone());
        }
        let (op, stats) = (node.name, node.stats);
        if node.modulation_only {
            match op.as_str() {
                "osc" => {
                    if let Some(Some(hz)) = node.inputs.first().copied().filter(|f| f.is_some_and(|f| f <= LFO_HZ)) {
                        suggestions.push(format!(
                            "an oscillator at {hz}hz only sets a parameter: `control {{ osc(..) }}` runs it once per block"
                        ));
                    }
                }
                "env" => suggestions.push(
                    "an envelope only sets a parameter: `control { env[..] }` runs it once per block (a segment shorter than a block becomes one step)".to_string(),
                ),
                _ => {}
            }
        }
        match stats {
            None => sample_ops.push(op),
            Some(stats) => {
                for (native, per_block) in &stats.natives {
                    if *per_block || stats.control_scope {
                        block_ops.push(native.clone());
                    } else {
                        sample_ops.push(native.clone());
                    }
                }
                kernels.push((op, stats));
            }
        }
    }
    let _ = writeln!(out, "{name}");
    let _ = writeln!(
        out,
        "  every sample: {} op{}{}",
        sample_ops.len(),
        if sample_ops.len() == 1 { "" } else { "s" },
        if sample_ops.is_empty() {
            String::new()
        } else {
            format!(" ({})", tally(&sample_ops))
        }
    );
    let _ = writeln!(
        out,
        "  once per block: {} op{}{}",
        block_ops.len(),
        if block_ops.len() == 1 { "" } else { "s" },
        if block_ops.is_empty() {
            String::new()
        } else {
            format!(" ({})", tally(&block_ops))
        }
    );
    for warning in warnings {
        let _ = writeln!(out, "  warning: {warning}");
    }
    for suggestion in suggestions {
        let _ = writeln!(out, "  could run once per block: {suggestion}");
    }
    for (op, stats) in kernels {
        let label = if stats.control_scope {
            "control scope".to_string()
        } else {
            format!("function `{op}`")
        };
        let _ = if stats.control_scope {
            writeln!(
                out,
                "    {label}: {} at note start, {} per block",
                stats.init,
                stats.block + stats.sample
            )
        } else {
            writeln!(
                out,
                "    {label}: {} at note start, {} per block, {} per sample",
                stats.init, stats.block, stats.sample
            )
        };
    }
    out.push('\n');
}
