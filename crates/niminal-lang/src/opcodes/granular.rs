//! Granular's named controls and immutable resources.
use super::*;
use niminal_engine::ops::GranularConfig;

fn one(a: &BuildArgs, name: &str) -> Result<Option<Arc<niminal_engine::Table>>, String> {
    match a.tables.get(name) {
        None => Ok(None),
        Some(tables) if tables.len() == 1 => Ok(Some(tables[0].clone())),
        _ => Err(format!("`{name}` needs one mono table")),
    }
}

fn whole(a: &BuildArgs, name: &str, default: usize, max: usize) -> Result<usize, String> {
    let value = a.consts.get(name).copied().unwrap_or(default as f64);
    if !value.is_finite() || value.fract() != 0.0 || value < 1.0 || value > max as f64 {
        return Err(format!("`{name}` must be a whole number from 1 to {max}"));
    }
    Ok(value as usize)
}

pub(super) fn spec() -> OpSpec {
    OpSpec::new(
        "granular",
        "Particle synthesis from four independently positioned/tuned source readers, a selectable mono source_bank and trainlets. Controls are captured at grain birth, including gain, probability, source weights, pitch sweep, envelopes and routing; fm remains live inside grains. Density is grains/second (20hz), duration defaults to 100ms, rate to recorded speed, and position wraps in 0..1. Freq sets cycle frequency in hz regardless of table length; omitted uses recorded sample rates. Rate1..rate4 independently scale each reader, including reverse and zero speed. Source2 blends with source via mix, or weight1..weight4 set independent signed gains; train_gain adds normalized pulse trains controlled by train_freq, train_partials and train_chroma. Position2..4 inherit position; pitch and pitch2..4 transpose each reader in semitones. Select..select4 choose source_bank entries at grain birth. Spray is a +/- time offset, pitch_scatter is +/- semitones, pan is -1..1, spread is random pan width. FM is relative: fm * captured fm_depth * fm_window modulates each reader's instantaneous playback speed. Fm_windows and fm_window_index can select a different captured FM envelope per grain. Pitch_start and pitch_end set sweep endpoints in semitones; pitch_end defaults to pitch_start + sweep. Sweep_shape in 0..1 runs from hold-start through linear-rate (0.5) to hold-end; omitted gives a geometric pitch sweep. Shape sets attack/decay balance, sustain the held fraction. Attack_window and decay_window shape those regions; window is a secondary full-grain envelope blended by window_amount; fm_window shapes modulation depth. Positive trigger edges add grains. Sync adds signed phase offsets; a value >=1 hard-syncs and holding it high pauses the clock. Distribution displaces grains around the clock using distribution_table values in periods (negative selects sequentially, positive randomly), capped at 10sec. Grains capture controls before displacement. Gain_mask, start_mask, end_mask, fm_mask and channel_mask use [loop_start, loop_end, values...]; mix_mask has five gains per entry (four sources and trainlet). Bounds are inclusive, zero-based. With mix_mask, omitted reader weights and train_gain default to 1 so the mask controls all five gains; otherwise source3/4 default to unity when present and trainlets default off. Gain_index, start_index, end_index, fm_index, channel_index and mix_index can set their cursors; masks advance even for rejected grains. Channel routes between neighboring circular outputs (0-based, fractional); pan_law holds eight concatenated curves. Channels is 1..8 (default 2); max_grains is 1..4096 (256), stealing the oldest on overload. Monitor: 1 returns a record with audio, sync, phase and six mask-index fields; audio is the normal result without monitoring. Probability preserves the clock and other random choices. Seed is constant. After release and envelopes finish, active grains drain and delayed grains are cancelled. Allow output headroom as overlap rises.",
        vec![
            req("source", Kind::Table),
            opt("density", Kind::Signal(Unit::Hz)),
            opt("duration", Kind::Signal(Unit::Time)),
            opt("position", SIGNAL),
            opt("rate", SIGNAL),
            opt("freq", Kind::Signal(Unit::Hz)),
            opt("rate1", SIGNAL),
            opt("rate2", SIGNAL),
            opt("rate3", SIGNAL),
            opt("rate4", SIGNAL),
            opt("spray", Kind::Signal(Unit::Time)),
            opt("pitch_scatter", SIGNAL),
            opt("jitter", SIGNAL),
            opt("trigger", SIGNAL),
            opt("shape", SIGNAL),
            opt("sweep", SIGNAL),
            opt("pan", SIGNAL),
            opt("spread", SIGNAL),
            opt("fm", SIGNAL),
            opt("fm_depth", SIGNAL),
            opt("source2", Kind::Table),
            opt("mix", SIGNAL),
            opt("gain", Kind::Gain),
            opt("probability", SIGNAL),
            opt("position2", SIGNAL),
            opt("pitch2", SIGNAL),
            opt("source3", Kind::Table),
            opt("source4", Kind::Table),
            opt("source_bank", Kind::Table),
            opt("sync", SIGNAL),
            opt("distribution", SIGNAL),
            opt("distribution_table", Kind::Table),
            opt("sustain", SIGNAL),
            opt("window_amount", SIGNAL),
            opt("sweep_shape", SIGNAL),
            opt("pitch_start", SIGNAL),
            opt("pitch_end", SIGNAL),
            opt("pitch", SIGNAL),
            opt("position3", SIGNAL),
            opt("pitch3", SIGNAL),
            opt("position4", SIGNAL),
            opt("pitch4", SIGNAL),
            opt("weight1", Kind::Gain),
            opt("weight2", Kind::Gain),
            opt("weight3", Kind::Gain),
            opt("weight4", Kind::Gain),
            opt("channel", SIGNAL),
            opt("train_freq", Kind::Signal(Unit::Hz)),
            opt("train_partials", SIGNAL),
            opt("train_chroma", SIGNAL),
            opt("train_gain", Kind::Gain),
            opt("select", SIGNAL),
            opt("select2", SIGNAL),
            opt("select3", SIGNAL),
            opt("select4", SIGNAL),
            opt("window", Kind::Table),
            opt("attack_window", Kind::Table),
            opt("decay_window", Kind::Table),
            opt("fm_window", Kind::Table),
            opt("fm_windows", Kind::Table),
            opt("fm_window_index", SIGNAL),
            opt("pan_law", Kind::Table),
            opt("gain_mask", Kind::Table),
            opt("start_mask", Kind::Table),
            opt("end_mask", Kind::Table),
            opt("fm_mask", Kind::Table),
            opt("channel_mask", Kind::Table),
            opt("mix_mask", Kind::Table),
            opt("gain_index", SIGNAL),
            opt("start_index", SIGNAL),
            opt("end_index", SIGNAL),
            opt("fm_index", SIGNAL),
            opt("channel_index", SIGNAL),
            opt("mix_index", SIGNAL),
            setting("channels", SIGNAL),
            setting("max_grains", SIGNAL),
            setting("monitor", SIGNAL),
            setting("seed", SIGNAL),
        ],
        Build::Builtin(|a| {
            let sources = ["source", "source2", "source3", "source4"]
                .map(|name| a.tables.get(name).cloned().unwrap_or_default());
            if a.consts
                .get("monitor")
                .is_some_and(|v| *v != 0.0 && *v != 1.0)
            {
                return Err("monitor must be 0 or 1".into());
            }
            let mut masks = std::array::from_fn(|_| None);
            for (out, name) in masks.iter_mut().zip([
                "gain_mask",
                "start_mask",
                "end_mask",
                "fm_mask",
                "channel_mask",
                "mix_mask",
            ]) {
                *out = one(a, name)?;
            }
            let config = GranularConfig {
                sources,
                source_bank: a.tables.get("source_bank").cloned().unwrap_or_default(),
                window: one(a, "window")?,
                attack_window: one(a, "attack_window")?,
                decay_window: one(a, "decay_window")?,
                fm_window: one(a, "fm_window")?,
                fm_windows: a.tables.get("fm_windows").cloned().unwrap_or_default(),
                distribution_table: one(a, "distribution_table")?,
                pan_law: one(a, "pan_law")?,
                masks,
                channels: whole(a, "channels", 2, 8)?,
                max_grains: whole(a, "max_grains", 256, 4096)?,
                monitor: a.consts.get("monitor").is_some_and(|v| *v == 1.0),
                seed: seed_of(a).unwrap_or(0),
            };
            Ok(Box::new(Granular::new(config)?))
        }),
    )
}
