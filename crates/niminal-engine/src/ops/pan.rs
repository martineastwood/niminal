use std::sync::Arc;

use crate::opcode::{Opcode, Port, ProcessCtx};

/// Gains for placing a mono source at `angle` degrees (0 is straight ahead,
/// positive to the right) among speakers at the given angles. `None` marks a
/// channel that is never panned to, such as an LFE.
///
/// A source is shared between the two nearest speakers with constant power. If
/// those two are more than 180 degrees apart (as the gap behind a stereo pair
/// is) the source goes to the nearer one alone. `spread` (0..1) blends towards
/// all speakers equally.
fn pan_gains_sorted(
    ring: &[(f32, usize)],
    angle: f32,
    spread: f32,
) -> [f32; crate::mixer::MAX_CHANNELS] {
    let mut gains = [0.0f32; crate::mixer::MAX_CHANNELS];
    if ring.is_empty() {
        return gains;
    }

    let az = angle.rem_euclid(360.0);
    // The speaker at or before the source going round the circle, and the next.
    let before = ring
        .iter()
        .rposition(|(a, _)| *a <= az)
        .unwrap_or(ring.len() - 1);
    let (a_angle, a_idx) = ring[before];
    let (b_angle, b_idx) = ring[(before + 1) % ring.len()];

    let gap = (b_angle - a_angle).rem_euclid(360.0);
    let gap = if ring.len() == 1 || gap == 0.0 {
        360.0
    } else {
        gap
    };
    let into = (az - a_angle).rem_euclid(360.0);

    if ring.len() == 1 {
        gains[a_idx] = 1.0;
    } else if gap > 180.0 {
        // No pair to share between: the nearer speaker takes it all.
        if into <= gap - into {
            gains[a_idx] = 1.0
        } else {
            gains[b_idx] = 1.0
        }
    } else {
        let t = (into / gap).clamp(0.0, 1.0);
        let (s, c) = (t * std::f32::consts::FRAC_PI_2).sin_cos();
        gains[a_idx] = c;
        gains[b_idx] = s;
    }

    let spread = spread.clamp(0.0, 1.0);
    if spread > 0.0 {
        let uniform_sq = 1.0 / ring.len() as f32;
        for &(_, i) in ring.iter() {
            gains[i] = ((1.0 - spread) * gains[i] * gains[i] + spread * uniform_sq).sqrt();
        }
    }
    gains
}

pub fn pan_gains(
    speakers: &[Option<f32>],
    angle: f32,
    spread: f32,
) -> [f32; crate::mixer::MAX_CHANNELS] {
    let mut slots = [(0.0f32, 0usize); crate::mixer::MAX_CHANNELS];
    let mut count = 0;
    for (i, a) in speakers.iter().enumerate().take(slots.len()) {
        if let Some(a) = a {
            slots[count] = (a.rem_euclid(360.0), i);
            count += 1;
        }
    }
    let ring = &mut slots[..count];
    ring.sort_by(|a, b| a.0.total_cmp(&b.0));
    pan_gains_sorted(ring, angle, spread)
}

/// One output channel of a pan: the mono input scaled by this speaker's gain
/// for the current angle. A pan to N speakers is N of these sharing inputs.
#[derive(Clone)]
pub struct PanChannel {
    /// Pre-sorted `(angle, channel_index)` ring, avoiding per-sample sorting under modulation.
    ring: Arc<[(f32, usize)]>,
    index: usize,
    /// The angle and spread the gain was last worked out for, and that gain. Panning rarely moves,
    /// and working it out is far dearer than the multiply.
    last: Option<(f32, f32, f32)>,
}

impl PanChannel {
    pub fn new(speakers: Arc<[Option<f32>]>, index: usize) -> Self {
        let mut ring: Vec<(f32, usize)> = speakers
            .iter()
            .enumerate()
            .take(crate::mixer::MAX_CHANNELS)
            .filter_map(|(i, a)| a.map(|a| (a.rem_euclid(360.0), i)))
            .collect();
        ring.sort_by(|a, b| a.0.total_cmp(&b.0));
        PanChannel {
            ring: ring.into(),
            index,
            last: None,
        }
    }
}

impl Opcode for PanChannel {
    fn name(&self) -> &str {
        "pan"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] = &[
            Port::required("x"),
            Port::optional("angle", 0.0),
            Port::optional("spread", 0.0),
        ];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(self.clone())
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            let (angle, spread) = (inputs[1][i], inputs[2][i]);
            let gain = match self.last {
                Some((a, s, gain))
                    if a.to_bits() == angle.to_bits() && s.to_bits() == spread.to_bits() =>
                {
                    gain
                }
                _ => {
                    let gain = pan_gains_sorted(&self.ring, angle, spread)[self.index];
                    self.last = Some((angle, spread, gain));
                    gain
                }
            };
            *o = inputs[0][i] * gain;
        }
    }
}
