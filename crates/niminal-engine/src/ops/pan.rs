use std::sync::Arc;

use crate::opcode::{Opcode, Port, ProcessCtx};

/// Gains for placing a mono source at `azimuth` degrees (0 is straight ahead,
/// positive to the right) among speakers at the given angles. `None` marks a
/// channel that is never panned to, such as an LFE.
///
/// A source is shared between the two nearest speakers with constant power. If
/// those two are more than 180 degrees apart (as the gap behind a stereo pair
/// is) the source goes to the nearer one alone. `spread` (0..1) blends towards
/// all speakers equally.
pub fn pan_gains(speakers: &[Option<f32>], azimuth: f32, spread: f32) -> [f32; crate::mixer::MAX_CHANNELS] {
    let mut gains = [0.0f32; crate::mixer::MAX_CHANNELS];
    // the speakers that can be panned to, by angle (no allocation: this runs for every sample of a pan)
    let mut slots = [(0.0f32, 0usize); crate::mixer::MAX_CHANNELS];
    let mut count = 0;
    for (i, a) in speakers.iter().enumerate().take(slots.len()) {
        if let Some(a) = a {
            slots[count] = (a.rem_euclid(360.0), i);
            count += 1;
        }
    }
    if count == 0 {
        return gains;
    }
    let ring = &mut slots[..count];
    ring.sort_by(|a, b| a.0.total_cmp(&b.0));

    let az = azimuth.rem_euclid(360.0);
    // The speaker at or before the source going round the circle, and the next.
    let before = ring.iter().rposition(|(a, _)| *a <= az).unwrap_or(ring.len() - 1);
    let (a_angle, a_idx) = ring[before];
    let (b_angle, b_idx) = ring[(before + 1) % ring.len()];

    let gap = (b_angle - a_angle).rem_euclid(360.0);
    let gap = if ring.len() == 1 || gap == 0.0 { 360.0 } else { gap };
    let into = (az - a_angle).rem_euclid(360.0);

    if ring.len() == 1 {
        gains[a_idx] = 1.0;
    } else if gap > 180.0 {
        // No pair to share between: the nearer speaker takes it all.
        if into <= gap - into { gains[a_idx] = 1.0 } else { gains[b_idx] = 1.0 }
    } else {
        let t = (into / gap).clamp(0.0, 1.0);
        gains[a_idx] = (t * std::f32::consts::FRAC_PI_2).cos();
        gains[b_idx] = (t * std::f32::consts::FRAC_PI_2).sin();
    }

    let spread = spread.clamp(0.0, 1.0);
    if spread > 0.0 {
        let uniform = 1.0 / (ring.len() as f32).sqrt();
        for &(_, i) in ring.iter() {
            gains[i] = ((1.0 - spread) * gains[i] * gains[i] + spread * uniform * uniform).sqrt();
        }
    }
    gains
}

/// One output channel of a pan: the mono input scaled by this speaker's gain
/// for the current azimuth. A pan to N speakers is N of these sharing inputs.
#[derive(Clone)]
pub struct PanChannel {
    speakers: Arc<[Option<f32>]>,
    index: usize,
    /// The azimuth and spread the gain was last worked out for, and that gain. Panning rarely moves,
    /// and working it out is far dearer than the multiply.
    last: Option<(f32, f32, f32)>,
}

impl PanChannel {
    pub fn new(speakers: Arc<[Option<f32>]>, index: usize) -> Self {
        PanChannel { speakers, index, last: None }
    }
}

impl Opcode for PanChannel {
    fn name(&self) -> &str {
        "pan"
    }

    fn ports(&self) -> &[Port] {
        const PORTS: &[Port] =
            &[Port::required("x"), Port::optional("azimuth", 0.0), Port::optional("spread", 0.0)];
        PORTS
    }

    fn box_clone(&self) -> Box<dyn Opcode> {
        Box::new(self.clone())
    }

    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            let (azimuth, spread) = (inputs[1][i], inputs[2][i]);
            let gain = match self.last {
                Some((a, s, gain)) if a.to_bits() == azimuth.to_bits() && s.to_bits() == spread.to_bits() => gain,
                _ => {
                    let gain = pan_gains(&self.speakers, azimuth, spread)[self.index];
                    self.last = Some((azimuth, spread, gain));
                    gain
                }
            };
            *o = inputs[0][i] * gain;
        }
    }
}
