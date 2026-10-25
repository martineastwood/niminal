---
title: Instruments
description: instr and mono, voices, out, parameters, and multichannel.
---

**Compiled by niminal.** An instrument is compiled by the server; a client only decides when to play it.

Instruments are templates; each note is a **voice**. An instrument is a function the score can call: everything in [Functions](/reference/language/functions/) applies, and its parameters are set by notes.

## Result: last expression or `out`

The result is the **last expression**, or **`out`** if the body uses `out`: `out` starts as silence and can be assigned or added to. Using both in one body is an error. The same rule applies to `fn`.

```nml
instr saw_lead(freq: hz, amp: db = -6db) {
  level = env[0 5ms 1 200ms 0.6 | 300ms 0]
  osc(saw, freq).lpf(cutoff: 2khz).gain(amp) * level
}

instr pad(freq: hz, amp: db = -3db) {
  out  = osc(saw, freq).lpf(cutoff: 1200hz).gain(amp)
  out += osc(sine, freq / 2).gain(amp - 6db)
  space += out.gain(-12db)
}
```

- Each `out +=` line is a layer; commenting one out mutes it.
- Instruments return `out` rather than writing to speakers — the caller decides routing.
- **Calling an instrument inside another** plays its body as part of this voice; `gain:` is applied after it, as for any note: `click(freq: freq) + click(freq: freq * 1.5, gain: -6db)`.
- **`y = x + prev(y) * 0.5`** is a feedback loop one sample long, as in a function; `prev(x)` of a name already defined is `x` a sample ago.

## Parameters

- Each needs a type or a default. Numeric parameters, fixed-shape arrays and `buffer` references can stay live. Choices, functions and explicitly `const` parameters specialize the graph and need a default; set those when calling the instrument inside another instrument.
- **Without a rate** a parameter stays live: lanes, glides, MPE and `n.set(...)` move it on the running voice. **`init`** fixes it at note start: `instr pluck(freq: init hz)`.
- **`gain`** is a control every note has, applied after the instrument. An instrument may declare its own `gain: db = 0db` to take it over and use it as it likes; then it isn't applied for it.

### Structural parameters

```nml
fn sine_voice(freq: hz) { osc(sine, freq) }

instr bank(freq: hz, partials: const [num] = [1, 0.5], make: fn = sine_voice,
           mode: (soft | hard) = soft) {
  signal = (for i in partials.indices { make(freq * (i + 1)) * partials[i] }).sum
  if mode == soft { signal.tanh } else { signal }
}

instr bright(freq: hz) {
  bank(freq, partials: [1, 0.7, 0.3], mode: hard)
}
```

These structural arguments select graph structure at compilation. They are omitted from the note/event control signature, so `set` cannot change them. A `const` untyped argument can also bind a named table or shared buffer, for example `source: const = take`. Redefine the instrument to change its structure live.

### Live arrays and buffer references

An ordinary array parameter needs a constant default to establish its shape. Its numeric elements become live controls. A `buffer` parameter needs a named shared buffer as its default:

```nml
buffer take = zeros(48000)
buffer alternate = zeros(48000)

instr sampler(weights: [num] = [1, 0.5], source: buffer = take) {
  source.at(note.time.num * sample_rate.num) * weights.sum
}

sampler(weights: [0.25, 0.5], source: alternate) for 100ms
```

Arrays keep their declared shape for the life of a voice. Numeric element types, units and ranges are checked. Use `const [num]` when the elements themselves must be known while compiling, such as a wavetable used by `osc`.

Buffer references read with `source[i]` or `source.at(pos, interp: linear)`, including through DSP functions and nested scopes. A running voice can select any shared buffer prepared with its original instrument graph. Buffers allocated later are available to newly prepared voices. `init buffer` captures the selection at note start. Reference parameters currently support reading, not recording or writes through the reference.

Clients update these controls with the timed `array` and `bind_buffer` events. Individual numeric elements also have paths such as `weights[0]`, accepted by `set` and `map`. See [Engine API](/reference/engine-api/#live-parameters-and-modulation).

## Oscillator controls

`osc` accepts `sine`, `saw`, `square`, `pulse`, `tri`, a wavetable, or a wavetable bank. Its numeric controls can be modulated at audio rate:

```nml
instr pwm(freq: hz) {
  width = 0.5 + osc(sine, 0.3hz) * 0.35
  out = osc(pulse, freq, width: width) * env[0 10ms 0.15 | 100ms 0]
}
```

`width` is the fraction of a cycle spent high. `pulse` clamps it to 0.01..0.99; at 0.5 it matches `square`. `square` stays at 50% duty cycle, and the other waveforms ignore width. Saw and pulse use polynomial band-limited step corrections.

### Phase and hard sync

```nml
osc(sine, 220hz, phase: 0.25)                // start a quarter-cycle ahead
osc(sine, 220hz, phase_mod: modulation)     // continuously offset the phase, in cycles
osc(saw, 660hz, sync: 220hz)                // slave 660hz, master resets at 220hz
```

`phase` is captured on the oscillator's first sample and wraps into 0..1, including negative values. A reset scope re-arms that capture. `phase_mod` stays live and offsets the waveform lookup; it does not change the running phase accumulator.

A positive `sync` is the **master frequency in hz**, not a trigger signal. Each master cycle resets the slave to its captured starting phase. Changing the slave frequency while holding the master fixed gives the classic hard-sync sweep. The master's phase starts at zero with each oscillator; sync is per voice. Zero or negative sync disables the master.

Resets retain their fractional position between samples. Polynomial corrections reduce the value and slope discontinuities at reset, including with tables and banks; this suppresses aliasing without adding an output delay. It does not guarantee alias-free output under arbitrary modulation. For aggressive sweeps or modulation, use the existing oversampling scope:

```nml
voice = oversample(4) { osc(saw, freq * ratio, sync: freq) }
```

In sync mode `osc` bounds the slave frequency to ±half the sample rate and the master to half the sample rate. `position` still morphs a wavetable bank. The existing phase-modulation and free-running oscillator behaviour is retained when the new controls are omitted.

### Phase ramps for custom readers

```nml
p = phasor(2hz, phase: 0.25)
y = curve.phase(p)
```

`phasor(freq, phase: 0, sync: 0hz)` produces a ramp in 0..1. `phase` and the master-frequency `sync` have the same meanings as on `osc`. Negative frequency runs the ramp backwards; zero frequency holds it. Its sync master is bounded to half the sample rate. Use it to scan a table or buffer, or build a custom modulation signal. A raw phase ramp and arbitrary table lookups are not band-limited audio waveforms; use `osc(table, freq)` for band-limited wavetable playback.

`examples/oscillator_controls.nml` combines a PWM pad, a hard-sync lead, and a phase ramp controlling panning.

## The running voice

| Name | Meaning |
| --- | --- |
| `note.time` | Time since the voice started, in sec |
| `note.released` | 0 while held, 1 after the release |
| `note.dur` | The length the note was scheduled for, in sec (0 for a note held from a key) |
| `note.start` | 1 for the first sample of the voice, then 0 |
| `note.index` | How many notes of this instrument have started before this one |
| `clock.time` | Elapsed engine time in seconds |
| `clock.beat`, `clock.bar`, `clock.bpm` | The live client’s transport: beats and bars since its performance started, and its tempo. The client lowers these to ordinary DSP controls; the server has no musical transport. |

Functions don't see `note` or `clock`: pass these in as arguments.

### Sharing state between notes

A shared [`buffer`](/reference/language/tables/#shared-buffers) is how one note sees another. A write in an `if` happens only while the branch is taken, so this counts the notes and lets each one read the count:

```nml
buffer count = zeros(1)
instr tick(freq: hz) {
  if note.start { count[0] = count[0] + 1 }
  osc(sine, freq * (1 + count[0] * 0.01)) * env[1 | 50ms 0]
}
```

`note.index` picks a slot for each note. A note that reads a buffer sees what earlier notes wrote.

### Delayed layers and ending early

`after(delay) { body }` plays `body` once the note is `delay` old, from fresh state, and is silent before: `out = pluck + after(1/8beat) { pluck.gain(-6db) }`. It is `reset_on(note.time >= delay) { body } * (note.time >= delay)`. `signal.finish_when(done)` ends a note from the inside.

For signal-triggered overlapping layers, use `spawn`:

```nml
instr cloud(freq: hz = 220hz) {
  spawn(osc(square, 20hz), dur: 80ms, max: 8) {
    osc(sine, freq) * env[0 2ms 1 | 20ms 0]
  }
}
```

Each positive trigger edge starts a fresh layer and captures the surrounding signal values it reads. `dur` is captured on that edge too. The layer has its own envelopes, state, `note.time`, `note.start`, `note.index`, and `note.released`. It releases after `dur`, then drains its envelopes and effects. Releasing the parent releases all its layers and stops new triggers. `max` is a compile-time integer from 1 to 64 (default 8); a full pool drops new triggers. Storage is prepared before playback.

The body can call an instrument or a DSP function, and can read a shared buffer with `buf[i]` or `buf.at(pos)`. It follows the sample DSP function rules: channel-layout DSP operations, bus sends, shared writes and OSC output belong outside the scope. `spawn`, `oversample` and `reset_on` scopes can compose; `spawn` and `oversample` nesting is limited to 16 levels. Use clips and `at` to schedule independent top-level notes.

### Noise

Every `noise()` in an instrument, and every note of it, has its own noise, and a render is the same every time. `noise(seed: 7)` gives the same noise to every note.

## When a voice ends

- With an envelope (or a sample), it lives until it is released and every envelope has finished, and anything they fed, such as a delay, has rung out.
- Custom functions can use `signal.finish_when(done)` to report completion just like an envelope; every layer must finish. See [Functions](/reference/language/functions/#custom-completion).
- Without one, it fades out at its release.
- **`tail = 2sec`** in the body keeps it sounding that long after its release (and after its envelopes are done), then it fades: what a plucked string or an endless oscillator into a delay needs.

## `instr` and `mono`

| Keyword | A new note… | Used by |
| --- | --- | --- |
| `instr` | Spawns a new voice (poly, default) | Most instruments |
| `mono` | Ends the voice before it and starts its own (one voice at a time, per clip name) | Bass, leads |

Continuous fx declarations and generators use **`fx`** (see [Routing](/reference/language/routing/)).

### Settings

Settings are written in the body as `name = value`, and are not part of the sound:

```nml
instr pad(freq: hz) {
  max       = 8
  same_note = release
  out       = …
}
```

A name that isn't one of the settings is an ordinary local value, so a misspelt setting is not reported.

| Setting | Effect | Built |
| --- | --- | --- |
| `choke = group` | New note ends other voices in the group, whichever instrument they are of | yes |
| `same_note = release` | New note releases the previous voice at the same pitch | yes |
| `max = n`, `steal = oldest` | Caps the voices of the instrument sounding on a clip; the oldest end, with a short fade | yes |
| `steal = quietest` | With `max`, ends the quietest voice instead of the oldest | yes |
| `legato = true` (mono) | A note that starts while another is held takes over its voice: envelopes carry on, only the pitch moves | yes |
| `priority = last`, `lowest`, or `highest` (mono, needs `legato`) | Which held note sounds; letting go of one goes back to another still held | yes |
| `glide = 80ms` (mono) | A change of pitch slides to the new note in this time, evenly in semitones | yes |
| `pitch = name` | The `hz` parameter that notes, clips and keys set (default `freq`): `pitch = tone` in `instr bass(tone: hz = 55hz)`. A note call names it directly: `bass(tone: a2)` | yes |
| `compensate = false` | Opt out of latency compensation | no |
| `tail = 2sec` | How long the voice keeps sounding after its release | yes |
| `quantize = 1beat` | When redefinitions land | yes |

A setting that isn't built is rejected with a message, not ignored. `mono` is `max = 1`: the previous note fades out over a few milliseconds as the new one starts. Voice policies and `quantize` belong to instrument definitions; calls such as `pluck(bright: 0.2)` supply note parameter defaults. Voices are counted per clip name, so two clips playing the same `mono` instrument each keep their own.

An explicitly declared `gain: db` or `pan: deg` parameter is wired inside the instrument, so its lanes can affect bus sends as well as the direct output.

### Playing a mono instrument

```nml
mono bass(freq: hz, amp: db = -6db) {
  legato   = true
  glide    = 60ms
  priority = lowest
  level = env[0 5ms 1 | 200ms 0]
  out = osc(saw, freq).lpf(cutoff: freq * 4).gain(amp) * level
}
```

- **Plain `mono`** retriggers: each note fades the last one out and starts its own envelopes.
- **`glide`** alone makes the new note start from the pitch the last one had reached and slide to its own. It works on the pitch, `freq`, so the instrument needs that parameter.
- **`legato = true`** keeps one voice for as long as notes overlap. A note that starts while another is held doesn't attack again: the envelopes carry on, the note's other parameters (`amp`, lanes) apply, and the pitch moves, with `glide` if there is one. When the newest note ends, the voice goes back to a note still held; when the last one ends, the voice releases. The voice remembers up to 8 held notes.
- **`priority`** picks which held note legato sounds. `last` (the default) is the newest; `lowest` and `highest` hold the lowest or highest pitch, so a higher note played over a held bass note joins it but isn't heard until the low one ends.

With keys from `midi.keys()` this plays like a mono synth: hold a note, roll a second one over it, let go of it, and the first comes back.

**Lifetimes:** a voice ends naturally (released, envelopes and tail done) or forcibly (choked, stolen, replaced), always with a short global fade.

**Parameters are signals.** With `legato = true` a new note updates parameters on the running voice, which is what carries `glide`. A keyboard's pitch bend and pressure set the `bend` and `pressure` parameters of the notes it holds (see [Performance](/reference/language/performance/)). MPE works the same way: `keys: midi.keys(mpe: true)` gives each note its own bend, pressure and a `slide: 0..1` parameter.

**Pitch:** parameter `freq` is pitch by convention — used by `same_note`, mono priority, `keys.note`, and unqualified `transpose`.

Runtime modulation can connect a running parameter to a named control or bus without recompiling the instrument; see [Engine API](/reference/engine-api/#live-parameters-and-modulation). Bus mappings read exactly one block behind, avoiding dynamic routing cycles.

No project-wide voice cap in `config` today. Per-instrument limits use the `max` and `steal` settings above.

## What you pass is what you get

| You pass | Behavior | Example |
| --- | --- | --- |
| Constant | Stays constant | `amp: -6db` |
| Pattern | Each note gets its own value, fixed at note start | `amp: [-6db -12db -9db]` |
| Control / live signal | Stays live; running notes follow it | `amp: volume` where `ctl volume = …` |

## Multichannel

An array of signals becomes multichannel audio when it reaches an output. Built-ins expand over arrays (`osc(saw, [f, f * 1.01])` produces two oscillators). Signals are arrays of channels with layout metadata: they support indexing, mapping and reductions directly. `.channels` gives an ordinary array of the channels, and `.sum` mixes them down. Destructure when you need the channels separately:

```nml
[l, r] = src
mid  = (l + r) * 0.5
side = ((l - r) * 0.5).gain(+3db)
out  = [mid + side, mid - side]
```

The compiler warns when a layout doesn’t match the destination. Use `.to_layout(...)` or `pan` when crossing layouts — see [Routing](/reference/language/routing/).
