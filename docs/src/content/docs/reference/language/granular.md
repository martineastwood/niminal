---
title: Granular synthesis
description: Four source readers, trainlets, grain masks, envelopes, routing and synchronization.
---

`granular(source, ...)` makes overlapping particles from sample data or numeric tables.

Sources and shaping tables are immutable resources chosen when compiling. Runtime signals control grain creation and select source/FM-window bank entries. Static resource edits rebuild the instance; signal edits can retain its running state.

## Grain lifetime and defaults

`density: 20hz` schedules twenty grain opportunities per second. `duration` defaults to `100ms` (bounded from two samples to 10 seconds). Each opportunity captures its parameters before any timing displacement. `gain` accepts a linear factor or dB, and may be negative. `probability` ranges from 0 to 1 and defaults to 1; it rejects whole grains without shifting the clock or reshuffling position, pitch or pan randomness. `gain: 0` also skips new grains.

`max_grains` is a compile-time limit from 1 to 4096, default 256. The pool includes waiting grains. If full, the oldest scheduled grain is replaced. It allocates no memory while processing. After release and the end of the note's envelopes, active grains drain and waiting grains are cancelled. Long/dense clouds need output headroom.

`seed` is constant and defaults to 0. Resetting a voice repeats its seeded sequences; compatible hot reload retains the clock, masks, randomness and waiting/playing grains.

## Four independently controlled readers

| Control | Meaning |
| --- | --- |
| `source`, `source2`, `source3`, `source4` | Mono tables or stereo channel pairs, such as `recording.data` |
| `position`, `position2`, `position3`, `position4` | Wrapped normalized start positions; readers 2–4 follow `position` when omitted |
| `rate` | Shared speed multiplier, default 1 |
| `rate1` … `rate4` | Per-reader speed multipliers, default 1; zero freezes that reader and negative values reverse it independently |
| `pitch`, `pitch2`, `pitch3`, `pitch4` | Additional per-reader transposition in semitones, default 0 |
| `freq` | Optional cycle frequency in Hz, independent of table length; omitted plays samples at their recorded rate |
| `spray` | Shared random time offset in ±seconds, limited to 10 seconds |
| `pitch_scatter` | Shared random ±semitone offset, up to 96 semitones |
| `weight1` … `weight4` | Independent signed source gains, linear or dB |
| `mix` | Convenient source/source2 blend when independent weights are omitted |

Without a mix mask, weights 1/2 default to `1-mix`/`mix`, source3/4 default to unity when present, and trainlets are off. Every source respects its own sample rate. Synthetic tables use the output sample rate. `freq` instead reads each whole table as one cycle: its frame increment before sweep and FM is `freq * table_length / output_sample_rate`, multiplied by the shared rate, reader rate and transposition. This is useful for consistently pitched oscillator grains and table morphing.

```nml
field = sample("field.wav")
voice = sample("voice.wav")
instr hybrid() {
  granular(field.data, source2: voice.data,
    position: phasor(0.05hz), position2: 0.37,
    rate2: -1, pitch2: -7, mix: 0.4,
    density: 35hz, duration: 160ms, spray: 20ms,
    spread: 0.8) * env[0 100ms 1 | 300ms 0] * 0.1
}
```

`source_bank` is an arbitrary array of **mono** tables. `select`, `select2`, `select3`, `select4` choose an entry for each reader at grain birth; indices wrap, including negative indices. Omitted selectors use the fixed source for that reader. The selected table is retained for the life of the grain. Stereo source files can be used in fixed reader slots; individual channels can be bank entries.

```nml
const waves = [harmonics([1], size: 256), harmonics([1, 0.5, 0.3], size: 512)]
instr morph() {
  granular([0], source_bank: waves, select: phasor(0.2hz) * 2,
    freq: 110hz, density: 40hz, duration: 100ms) * env[1 | 100ms 0] * 0.1
}
```

## Envelopes and pitch sweeps

`shape` is the attack share of attack+decay time, default 0.5. `sustain` is the held share of total grain duration, default 0. `attack_window` runs over the attack and `decay_window` over the decay. The sustain level is the final attack-table value. Tables are read linearly, including their endpoints. Both time fractions are captured per grain.

Without custom tables the primary envelope is raised-cosine. Supplying `window` gives a secondary full-grain envelope; primary regions without an explicit attack/decay table then default to flat, preserving the simple custom-window behavior. `window_amount` (0..1, default 1) blends this secondary envelope with a flat window, then multiplies the primary envelope. Windows need at least two finite values; zero outer endpoints avoid clicks.

`pitch_start` and `pitch_end` are semitone offsets over each grain's lifetime. `pitch_end` defaults to `pitch_start + sweep`. A finite `sweep_shape` sets rate-curve curvature: 0 holds the start, 0.5 is linear in playback rate, and 1 holds the end. Values between bend the sweep toward either end. Omitting it gives a geometric pitch sweep. Start/end masks multiply the corresponding frequency ratios.

FM remains live within active grains. Playback increment is:

```text
instantaneous_swept_increment * (1 + fm * captured_fm_depth * fm_envelope)
```

`fm_window` shapes modulation depth over grain age; omitted means constant depth. `fm_windows` is a bank of envelopes and `fm_window_index` captures a bank entry per grain. `fm_mask` scales the captured depth. This modulation applies to all readers and trainlets. An FM factor below -1 reverses the read direction.

## Trainlets

`train_gain` adds a finite cosine pulse train within each grain. `train_freq` is its fundamental (default 100hz), `train_partials` controls its harmonic count (default 16), and `train_chroma` controls the ratio between successive harmonic weights (1 gives equal weights, below 1 favors low partials, above 1 favors high partials). Signed chroma alternates harmonic polarity. The trainlet fundamental is clamped to Nyquist. Spectra are peak-normalized and the partial count is limited by the highest sweep frequency's Nyquist bound. Very deep FM can still produce aliased sidebands.

Trainlets share the grain envelope, FM envelope, pitch sweep, gain and output routing. Their synthesis uses a closed-form harmonic sum, so adding partials does not add a per-partial rendering loop.

```nml
instr pulses() {
  granular([0], weight1: 0, train_gain: 0.5,
    train_freq: 110hz, train_partials: 30, train_chroma: 1.2,
    density: 25hz, duration: 150ms,
    pitch_end: 12, sweep_shape: 0.8,
    fm: osc(sine, 71hz), fm_depth: 0.4, fm_window: [1, 0.2, 0],
    spread: 1) * env[0 100ms 1 | 200ms 0] * 0.15
}
```

## Clock, displacement and synchronization

`trigger` adds a grain on a positive edge, including at `density: 0hz`. The internal clock and an external trigger on the same sample produce one opportunity. `jitter` (0..1) varies clock intervals and can accumulate timing drift.

For a steady underlying pulse, use `distribution` and `distribution_table`. Table values are nonnegative delays measured in grain periods. Positive distribution chooses random table entries; negative distribution reads sequentially. Magnitude scales the displacement. For example, density `10hz`, distribution `-1`, and table `[0, 0.5, 0.25]` starts grains at 0, 150, 225, 300… ms while clock pulses remain at 0, 100, 200, 300… ms. Negative delays clamp to zero, displacement is capped at ten seconds, and it is disabled below 0.001hz. All grain settings are captured at the clock pulse, before waiting. Fractional-sample birth positions are retained.

`sync` adds a signed offset to clock phase. A value >=1 hard-syncs and creates an opportunity; holding it high pauses the clock without stopping existing grains. Smaller positive/negative values advance/retard phase, clamped within its cycle. This also works with zero density, allowing an external signal to drive phase.

## Mask sequences and cursors

Masks run at grain rate, including opportunities rejected by probability or gain. Each table starts with **inclusive zero-based loop start/end indices**, followed by values:

```nml
const accents = [0, 3, 1, 0.3, 0, 0.7] // four entries, loop 0 through 3
```

| Table | Values after its two loop bounds |
| --- | --- |
| `gain_mask` | Signed grain gain, default 1 |
| `start_mask`, `end_mask` | Sweep endpoint frequency ratios, default 1 |
| `fm_mask` | FM-depth multiplier, default 1 |
| `channel_mask` | Circular output position |
| `mix_mask` | Groups of five gains: sources 1–4 and trainlet |

With a mix mask, omitted source weights and train gain default to 1, so the mask can select any of the five components directly. Explicit weight controls multiply its values. Unused trailing table padding is ignored; loop bounds must refer to complete entries.

`gain_index`, `start_index`, `end_index`, `fm_index`, `channel_index`, `mix_index` can set each cursor. A finite signal writes its index every sample; omitting the input lets it advance normally. Before reading, an out-of-loop cursor wraps to loop start. The cursor increments after a read, so its monitor output is the **next cursor**, which may be one past the inclusive loop end until the next read.

`monitor: 1` returns a record:

- `audio`: scalar for one output, an audio channel array otherwise.
- `sync`: the nominal scheduler/trigger pulse, even for a delayed or rejected grain.
- `phase`: scheduler phase, before that sample's phase increment; with jitter it is normalized to the current interval.
- The six named mask cursor outputs above.

Select `.audio` when rendering a monitored instance. Clock and cursor signals can be wired to another cloud. To align the entry just read on a shared pulse, use the leading cursor minus one:

```nml
leader = granular(sound.data, density: 12hz, gain_mask: accents, monitor: 1)
follower = granular(other.data, density: 0hz, sync: leader.sync,
  gain_mask: accents, gain_index: leader.gain_index - 1)
```

Inside a stateful function, monitor outputs can update state passed back to cursor inputs on the next sample.

## One to eight outputs

`channels` is a compile-time count from 1 to 8, default 2. Without explicit channel routing, the stereo default uses `pan` (-1..1) and per-grain `spread`, with equal-power mono panning and unity stereo balance at center.

`channel` and `channel_mask` route between neighboring outputs around a ring. Integer 0 sends to the first output, 1 to the second, and fractional positions share neighboring outputs. Positions wrap; on four outputs, 3.5 shares the fourth and first. Default channel routing is linear. Stereo source data is averaged to mono for explicit ring routing; the default stereo balance retains its channels.

`pan_law` holds **eight concatenated, equal-length curves**, each at least two values, describing gains from 0 to 1 across that neighboring pair. The earlier output reads the curve backwards and its neighbor reads it forwards. For equal power, use eight repeated sine-quarter curves. Gains are not automatically normalized. `examples/granular.nml` shows one way to generate this table.

For a surround layout, use a matching configuration or explicitly convert the unnamed output channels to a named layout such as `quad` before downmixing. Monitor fields are separate signals and must not be mixed into speaker outputs.
