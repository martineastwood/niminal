# Curating the standard library

The shelves are editable sounds with useful defaults and a small set of controls.
Stateful DSP primitives belong in the engine when native implementation improves
robustness or performance. Chosen oscillator mixtures, resonator ratios, envelopes,
velocity mappings and effect routing belong in NML. Do not promote a recipe to an
opcode just because it is reused.

A preset must stand alone when copied with the helpers listed in its catalog entry.
Each exposed control should affect the default sound. `gain` must make zero-gain
notes silent; if a patch declares its own gain parameter it takes responsibility
for applying it to the output, as well as any velocity-to-timbre mapping. In the
drum shelf, c3 is reference tuning and `decay` must change the audible tail.

Run the stdlib, preset_quality and stability tests after editing. The quality tests
check gain silence, softer notes, default levels and release in two registers,
audible drum controls, tail length and independent/mixed extreme settings. They
do not replace listening. Keep the existing safety suites intact and run
`scripts/check.sh --test` before shipping (`--apps` for app/editor changes).

Render a listening catalog:

    cargo run -p niminal-lang --example audition -- target/audition

Open target/audition/index.html. Default, soft, short and long notes and melodic
chords share one preview gain per preset; report.csv retains the raw measurements.
Listen for attacks, release clicks, aliasing, tuning, control usefulness and how
sounds sit together. Do not describe the collection as listening-approved merely
because the numerical tests pass.

Rebuild the original starter recordings after deliberately changing their patches:

    cargo run -p niminal-lang --example audition -- --starter crates/niminal-lang/assets/starter

The command refreshes WAVs, standalone source patches and starter.nml; README.md
and LICENSE are maintained alongside them. Source and audio live in niminal-lang and ship inside every host, including Stage
and the CLI. `use std/samples.{drums, electric_piano}` loads selected recordings
from memory, without extraction. The export command uses the same embedded bytes.
Imports retain the private members of a selected kit while omitting unrelated
recordings; a sample kit must not shadow a synth with the same public name.

## Audio changes in the 0.3 curation

Before refreshing references, the example baseline identified the intentionally changed
renders. Comparing the final references to the original ones gives the following changes
(maximum aligned 1024-sample RMS block change per channel, and total digest energy change):

| Example | Max block RMS change | Digest energy change |
| --- | ---: | ---: |
| clip_send.nml | 0.454892 | +6.530 dB |
| drum_voice.nml | 0.235675 | +1.704 dB |
| generated_drums.nml | 0.001338 | +0.001 dB |

The stronger 808 body, revised cowbell, working drum gain and shared snare decay
account for these changes. The scripted live baseline and the other existing
examples remained within their audio tolerances. preset_keys.nml is a new example.
The language version is now 0.3 for the preset signatures, removed introductory
drum names, native one-poles and renamed spectral/delay helpers; the user-facing
migration notes are in the standard-library reference.

## Harmonic scanning and stereo echo blocks

`scan_weights` and `scan_mix` provide a normalized moving window over signals.
`harmonic_bank` exposes mixed and individual partial outputs, with centre, width
and focus controls; `harmonic_osc` returns its mix. `stereo_echo` combines two
independent feedback delays with colour, timing skew, output diffusion, motion
and ducking. These are NML compositions of existing DSP primitives.

The revised `examples/rig.nml` demonstrates harmonic scanning, a separately panned
partial highlight, moving stereo echoes, and a breathing reverb. `contour` supplies
triggered or cycling rise/fall shapes and sample pulses for chaining; `space_reverb`
provides tank motion, absorption, diffusion, return tone and stereo-aware ducking.
Echo ducking also follows the louder input channel, avoiding cancellation with
opposite-polarity stereo input.

Before refreshing the first scanning reference, the baseline measured a maximum
aligned 1024-sample RMS block change of 0.058004 and a digest energy change of
+1.274 dB. The subsequent contour/reverb revision measured 0.225505 and +1.788 dB
against that scanning reference. Comparing the final reference to the original
one gives a maximum block RMS change of 0.162508 and a digest energy change of
+3.062 dB. The final rendered tail grew by 16,157 samples compared with the
original. The other examples and scripted live session remained within their
tolerances. These numerical checks do not constitute listening approval.

## Voice modules (language 0.4)

`multimode_filter`, `low_pass_gate`, `complex_osc`, `shift_random`, `texture`, `quantize`, `vocoder`, `cross_mod`, `slew`,
`morph_osc`, `wavefolder`, `multi_lfo`, `formant_filter`, `random_source`, `multi_voice`, `track_hold`, `comparator`, `chaos`, `rungler`, `resonant_body`, `shimmer_echo`, `freq_shifter`, `vca_mix`, `clock_mult`, `bernoulli`, `burst`, `gate_length`, `gate_delay`, `gate_logic`, `flip_flop`, `window_comparator`, `mux`, `stepper` and `sync_osc` are written to one contract, which later modules should follow:

- Every parameter takes a signal. Pitches and times are in their units, timbre controls run 0..1.
- Every input is clamped inside the block, so any value, including NaN or infinity, gives a finite output.
  Add each new block to `the_voice_modules_stay_finite_under_extreme_modulation` (or
  `the_processing_modules_stay_finite_under_extreme_modulation`) in `tests/blocks.rs`, and to the no-allocation
  tests in `tests/arrays_no_alloc.rs`.
- Outputs sit near ±1 for a full-scale input, so blocks chain without gain staging. Say in the doc comment
  where one can go higher (resonance, folding) and where it stops.
- Several jacks are several named results (`-> (low, band, high, notch)`). Results may share a name with
  a control; private locals still start with `_`.
- Called with only its required inputs, a block makes a sound worth hearing.
- Don't write the word `drum` (or other names the prelude defines, such as `lerp`) in a std comment or name: the prelude adds its own `drum` function to any file that mentions it, and its locals then clash with a user function called `voice`. Say `percussion`.

A shelf may `use std/blocks`. A preset taken from it brings only the blocks it calls (the expander
prunes a module that only other modules use), and the catalog copies those blocks with the preset. A
preset's own parameter cannot share a name with a block it calls: `rubber_bass` uses `perc` rather than
`contour` because `fm_keys` has a `contour` control.

The filter effects `lowpass`, `highpass`, `bandpass` and `band_cut` became one `filter` built on
`multimode_filter`, with a `response` control. The new patches are `west_coast_pluck`,
`harmonic_drone` and `rubber_bass` in `synths/modular`. Numerical tests pass for all of them; they
have not been listened to.

A stateful block that keeps per-channel state writes `state _s = x * 0`: zeros shaped like its input, so it filters
stereo channel by channel. That works however `x` was made, including as a signal computed inside the calling
function or another block.
