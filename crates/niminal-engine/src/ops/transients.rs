//! Offline amplitude-onset detection, shared by slicing and transient-aware playback.

/// Source frame positions, always including 0, separated by at least 60 ms.
pub fn detect_transients(samples: &[f32], rate: f32, max: usize) -> Vec<usize> {
    detect_transients_at(samples, rate, max, 0.5)
}

/// Like [`detect_transients`], with a `sensitivity` from 0 to 1: 0.5 is the usual, higher finds softer hits (and
/// more of them), lower only the clearest.
pub fn detect_transients_at(
    samples: &[f32],
    rate: f32,
    max: usize,
    sensitivity: f32,
) -> Vec<usize> {
    let s = sensitivity.clamp(0.0, 1.0);
    // how far a hit must jump above what came before it, and how loud it must be next to the loudest part
    let (jump, floor) = if s < 0.5 {
        (
            2.5 * (6.0f32 / 2.5).powf((0.5 - s) * 2.0),
            0.1 * 2.0f32.powf((0.5 - s) * 2.0),
        )
    } else {
        (
            2.5 * (1.4f32 / 2.5).powf((s - 0.5) * 2.0),
            0.1 * 0.5f32.powf((s - 0.5) * 2.0),
        )
    };
    let window = ((rate * 0.005) as usize).max(16);
    let hop = window / 2;
    let energy: Vec<f32> = (0..samples.len().saturating_sub(window) / hop + 1)
        .map(|i| {
            let frame = &samples[i * hop..(i * hop + window).min(samples.len())];
            (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt()
        })
        .collect();
    let loudest = energy.iter().copied().fold(0.0, f32::max);
    if loudest <= 0.0 {
        return vec![0];
    }
    let gap = (rate * 0.06) as usize;
    let mut found: Vec<(usize, f32)> = Vec::new();
    for i in 1..energy.len() {
        let before = &energy[i.saturating_sub(6)..i];
        let recent = before.iter().sum::<f32>() / before.len() as f32;
        // a hit is a jump to well above what came just before, and to a level that is not noise
        let strength = energy[i] / (recent + 0.02 * loudest);
        if strength < jump || energy[i] < floor * loudest || energy[i] < energy[i - 1] {
            continue;
        }
        // it began where the last quiet frame before the jump ends, a millisecond early so the attack is whole
        let quiet = (i.saturating_sub(4)..i)
            .rev()
            .find(|&frame| energy[frame] < 0.25 * energy[i]);
        let at = quiet
            .map_or(i * hop, |frame| frame * hop + window)
            .saturating_sub((rate * 0.001) as usize);
        if found.last().is_some_and(|&(last, _)| at < last + gap) {
            continue;
        }
        found.push((at, strength));
    }
    // a hit right at the start is the start
    found.retain(|&(at, _)| at > gap / 2);
    if found.len() + 1 > max {
        found.sort_by(|a, b| b.1.total_cmp(&a.1));
        found.truncate(max.saturating_sub(1));
        found.sort_by_key(|&(at, _)| at);
    }
    std::iter::once(0)
        .chain(found.into_iter().map(|(at, _)| at))
        .collect()
}
