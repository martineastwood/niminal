#![allow(dead_code)]
use std::sync::Arc;

use niminal_engine::{BLOCK, Graph, Voice};

pub const SR: f32 = 48_000.0;

/// Render `frames` samples in chunks of `chunk`, releasing the voice after
/// `release_at` samples (sample-accurately, by splitting chunks).
pub fn render(graph: &Arc<Graph>, frames: usize, release_at: Option<usize>, chunk: usize) -> Vec<f32> {
    let mut voice = Voice::new(graph.clone(), SR);
    let mut out = vec![0.0; frames];
    let mut pos = 0;
    while pos < frames {
        let mut n = chunk.min(BLOCK).min(frames - pos);
        if let Some(r) = release_at {
            if pos < r {
                n = n.min(r - pos);
            }
            if pos == r {
                voice.release();
            }
        }
        voice.process(&mut out[pos..pos + n]);
        pos += n;
    }
    out
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}
