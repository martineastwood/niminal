# Csound capabilities worth considering for Niminal

Review date: 2026-11-03. Scope: the language, Stage, and Tracker. Recommendations below are product judgments based on the Csound manual and this repository; they are not a proposal for opcode compatibility or comprehensive parity. Costs are relative engineering scope, not time estimates.

## Recommendation

Prioritize a small audio-analysis API and better ways to use the engine capabilities already present. Add two expressive physical models for new sounds. Keep the existing Niminal vocabulary, parameter metadata, deterministic rendering, and common engine shared by both UIs.

| Priority | Opportunity | Actual gap | Useful first delivery | Scope |
| --- | --- | --- | --- | --- |
| 1 | Pitch tracking and audio-derived controls | No dedicated fundamental-frequency tracker found; partial tracking is not F0 estimation | Monophonic pitch + confidence/voicing + level; sample root suggestion and Lab tuner | Medium DSP + UI |
| 2 | Record/resample directly into a playable sound | Engine recording/capture exists; the UIs lack a complete arm → capture → instrument workflow | Record an input or track for N bars, audition, trim, save to project, create sampler | Medium integration |
| 3 | Expressive bowed-string and reed models | Existing physical library covers plucked strings and struck resonators, not sustained bow/reed excitation | One bowed-string and one reed instrument with pressure, brightness and position controls | Medium/high DSP |
| 4 | Curved automation and editable modulation shapes | Language envelopes already have curves and loops; Tracker automation is less expressive | Curvature handles in Tracker, saved per segment and rendered identically offline | Small/medium UI + sequencing |
| 5 | Sound inspection in Lab and instrument editing | Tracker has scopes and the server has scope subscriptions; spectrum/tuner workflows are missing | Select a source, see spectrum and pitch, compare an effect before/after | Medium UI; reuse engine APIs |
| 6 | Ableton Link | MIDI clock exists, but no Ableton Link integration found | Shared tempo/beat phase in the sequencer, exposed as one sync choice in Stage and Tracker | Medium/high transport integration |
| 7 | Low-latency frequency shifting | FFT-based `spectral_shift` exists; a quadrature/IIR alternative is missing | One musical Hz-shifter effect, including stereo offset and wet/dry controls | Medium DSP |
| 8 | LPC voice/body modeling | Spectral envelope transfer already exists; LPC would be a different all-pole model | Experimental voice-to-instrument/body-transfer effect after pitch analysis | High DSP; defer |

## 1. Pitch and audio-derived control

Csound offers [ptrack](https://csound.com/docs/manual/ptrack.html) and [pitchamdf](https://csound.com/docs/manual/pitchamdf.html) for monophonic pitch analysis, plus descriptors such as [centroid](https://csound.com/docs/manual/centroid.html). These turn audio into useful musical control rather than merely another sound processor.

Niminal already has `follow`, offline onset detection for slicing, FFT/frame processing, and tracked partials. The missing product is a stable, documented F0 estimate that distinguishes voiced input from noise. `partial_morph` explicitly uses the lowest audible track as an anchor, not a fundamental detector.

Start with pitch in Hz, confidence/voicing, and level. Expose range and analysis latency, and define behavior on silence, noise and missing fundamentals. Retain the last good estimate only as an explicit policy. Let Stage show a tuner and let Tracker suggest a sample's root note. Later add a live onset trigger and brightness descriptor for audio-reactive instruments and modulation.

This needs a tested algorithm, not just a short FFT-bin maximum. Low notes require longer observation; fast response and low-frequency accuracy trade off. DSP-local triggers can drive sound directly. Reports sent back to a client cannot provide the same timing guarantees as an audio-thread trigger, so audio-to-pattern recording must account for analysis and transport latency.

## 2. Record/resample as a user workflow

Csound's [tablewa](https://csound.com/docs/manual/tablewa.html) and [flooper2](https://csound.com/docs/manual/flooper2.html) illustrate recording into tables and flexible loop playback. Niminal already has shared buffers, `buf.record`, audio input, buffer load/save, server capture, sample loop regions and crossfades. A replacement looper engine would duplicate much of that.

The valuable gap is in the UIs: choose input/track/master, arm a bar count, capture, audition, trim, and make the result an instrument with its source saved into the project. Stage can capture an improvisation into a new sample binding; Tracker can resample a synth or FX chain into a new instrument slot. Keep Stage's existing take/session-log recording as a distinct operation.

First delivery should be one-shot, quantized capture. Overdub, undo layers, punch-in and live loop performance can follow. Store the captured audio as a project asset so future playback/export does not depend on a live microphone or an ephemeral buffer. Define whether capture is pre/post FX and pre/post fader.

## 3. A small expressive physical-model family

Csound's [wgbow](https://csound.com/docs/manual/wgbow.html) exposes bow pressure and position; [wgclar](https://csound.com/docs/manual/wgclar.html) provides a nonlinear reed model. These are useful examples of sustained excitation that responds to performance gestures.

Niminal's `std/physical` already has a Karplus–Strong string and membrane, bar and bell resonators. Two sustained models would add more musical range than many more pluck or reverb variants. Prefer honest, controllable instruments over a long list of instrument-name approximations.

Ranged pressure and position parameters would work naturally in Stage's Lab and Tracker's generated controls/automation; Stage's existing MPE support can drive expression. Keep the first models compact and test pitch, low-frequency limits, sample-rate consistency, extreme controls, abrupt modulation, and release. Nonlinear feedback needs the same finiteness and live-level discipline as the filters just reviewed.

## 4. Curves and modulation editing

Csound has curved release envelopes such as [transegr](https://csound.com/docs/manual/transegr.html) and retriggerable shapes such as [loopseg](https://csound.com/docs/manual/loopseg.html). Niminal already supports curved durations, sustain/release, and looping envelope literals. Do not add another envelope syntax to imitate these opcodes.

Tracker's own manual lists curved envelope interpolation as missing. Add curve handles and retain the same physical-unit interpretation as its existing automation: geometric frequency/time motion and dB level motion where appropriate. Share segment evaluation between playback and offline export. Stage's corresponding opportunity is a visual view/editor of an existing envelope expression, with round-trip code editing; that is a larger follow-up, not a requirement for the Tracker improvement.

Keep note envelopes and pattern automation distinct: they differ in timing, retrigger and release semantics. A drawn curve must not imply an envelope can dynamically re-read controls that the language currently captures at note start.

## 5. Sound inspection

Csound's [dispfft](https://csound.com/docs/manual/dispfft.html) makes spectral inspection available while designing instruments. Niminal already has server `scopes.create`, Tracker's master/strip waveform scopes, meters and health reporting. Build on those rather than adding drawing opcodes to instrument source.

Stage Lab and Tracker's instrument/effect views should let a user select what they are observing, see a spectrum, and compare dry/wet levels. Add a tuner when pitch analysis is available. This directly helps users judge filter sweeps, aliasing, bass content and dynamics while editing code. Start with spectrum + tuner; a spectrogram, compressor gain-reduction trace, or measured filter response can follow.

Perform display work away from the audio callback, subscribe only while visible, and distinguish a scope snapshot from a continuous measurement stream. FFT displays must report window size/resolution honestly, especially for bass.

## 6. Ableton Link

Csound's [Ableton Link opcodes](https://csound.com/docs/manual/link_beat_get.html) expose session beat and phase; they are a separately installed plugin, not a mandatory core Csound facility. Niminal already supports MIDI clock, so this is an interoperability addition rather than a missing musical clock.

Its value is joining Live, drum apps, and other performers without configuring MIDI clock routing. Own the integration in the sequencer/transport, not inside individual instruments or the server's absolute-sample event model. Preserve quantized launches and continuity when tempo changes. Define phase, start/stop policy, leader/follower behavior, disconnect/rejoin behavior, and offline-render behavior. Stage and Tracker should expose the same clock-source choice.

Useful for live collaboration, but more integration and testing than adding an opcode. Rank it behind sound creation/editing unless synchronized ensemble performance is a near-term product priority.

## 7. A low-latency Hz shifter

Csound's IIR [hilbert](https://csound.com/docs/manual/hilbert.html) provides quadrature signals for techniques including single-sideband frequency shifting. Niminal's `spectral_shift` already shifts bins in Hz; the difference would be latency and character for live input, not the existence of frequency shifting itself.

A user-facing frequency shifter is more useful initially than asking users to assemble a Hilbert network. Use Hz controls, signed shifts, dry/wet, and optional stereo offset. Test unwanted-sideband rejection across the useful band, DC behavior, phase continuity, sample-rate scaling and modulation. IIR quadrature is an approximation and has frequency-dependent phase; it should not be advertised as exact or zero-latency across all frequencies.

## 8. LPC: a later, distinct sound-design option

Csound's [lpcanal](https://csound.com/docs/manual/lpcanal.html) and [lpcfilter](https://csound.com/docs/manual/lpcfilter.html) implement streaming prediction analysis and filtering. This can model voice/body resonances differently from STFT envelope transfer.

Niminal already ships `std/spectral.envelope_transfer`, formant processing, spectral pitch/formant control and partial morphing. A generic "add a vocoder" recommendation would miss those capabilities. LPC is worth an experiment only if its sound is clearly useful in comparison. Silent/ill-conditioned analysis, coefficient transitions, stability, analysis latency and output-level bounds all need careful handling.

## Already covered: do not put these on a missing-features list

- Band-limited basic oscillators, phase modulation, PWM and oscillator sync.
- Morphing wavetable banks, imported cycle frames, additive/inharmonic table constructors, lookup curves and deterministic table distributions.
- Granular/particle synthesis with multiple sources, trainlets, per-grain controls and modulation.
- FFT frames, spectral freezing/memory, pitch/formant control, spectral and partial morphing, thinning and component selection.
- Basic convolution, reverbs, filters, dynamics, delays and shared routing.
- Sample fitting, granular/WSOLA/transient-aware stretching, slices, SFZ, loops and loop crossfades.
- Curved/looped language envelopes, macros, per-parameter modulation, MIDI/OSC, MPE in the live sequencer, and Scala file loading.
- Offline rendering, session replay, stems, scopes and engine capture/buffer APIs. Some of these have opportunities for better UI exposure, not absent DSP.

## Keep out of scope for now

Csound compatibility, numeric instrument/table IDs, its score/orchestra split, FLTK widgets, duplicate routing systems, Python opcodes, exhaustive legacy analysis formats, a full STK catalog, plugin hosting, and broad ambisonic/HRTF support. Each would add substantial surface area without an established need in the current language, Stage or Tracker workflows. Existing multichannel layouts do not require a general spatial-audio platform.

## Practical delivery order

1. Curved Tracker automation and a spectrum view: relatively contained improvements using existing foundations.
2. Input/track capture to a saved sampler instrument: a complete creative workflow.
3. Pitch analysis shared by a tuner, sample root suggestion and audio-reactive instruments.
4. Bowed and reed models, delivered as ready-to-play sounds with a few musical controls.
5. Link when live synchronization becomes a priority; low-latency frequency shifting when live-input FX demand it.
6. Evaluate LPC against the existing envelope-transfer effect before committing to another analysis subsystem.

The priority table ranks user capability; this delivery order also considers dependency and integration cost. No implementation of these Csound-inspired additions is included in this review.
