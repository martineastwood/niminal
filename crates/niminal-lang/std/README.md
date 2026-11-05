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
and LICENSE are maintained alongside them. Source and audio ship inside the CLI,
so `niminal samples --out DIR` works without a repository or network connection.

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
