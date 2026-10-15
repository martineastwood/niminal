---
title: Instruments
description: instr and mono, voices, out, parameters, and multichannel.
---

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

- Each needs a type or a default. They can't be `const` (notes set them while playing), a choice, a list or a function.
- **Without a rate** a parameter stays live: lanes, glides, MPE and `n.set(...)` move it on the running voice. **`init`** fixes it at note start: `instr pluck(freq: init hz)`.
- **`gain`** is a control every note has, applied after the instrument. An instrument may declare its own `gain: db = 0db` to take it over and use it as it likes; then it isn't applied for it.

## The running voice

`note.time` is the time since the voice started (in sec) and `note.released` is 0 while held and 1 after the release. Functions don't see `note`: pass these in as arguments.

## When a voice ends

- With an envelope (or a sample), it lives until it is released and every envelope has finished, and anything they fed, such as a delay, has rung out.
- Custom functions can use `signal.finish_when(done)` to report completion just like an envelope; every layer must finish. See [Functions](/reference/language/functions/#custom-completion).
- Without one, it fades out at its release.
- **`with(tail: 2sec)`** keeps it sounding that long after its release (and after its envelopes are done), then it fades: what a plucked string or an endless oscillator into a delay needs.

## `instr` and `mono`

| Keyword | A new note… | Used by |
| --- | --- | --- |
| `instr` | Spawns a new voice (poly, default) | Most instruments |
| `mono` | Ends the voice before it and starts its own (one voice at a time, per part) | Bass, leads |

Continuous processors and generators are **tracks** (see [Performance](/reference/language/performance/)), not instruments.

### `with(...)` options

Options follow the parameters: `instr pad(freq: hz) with(max: 8, same_note: release) { … }`.

| Option | Effect | Built |
| --- | --- | --- |
| `choke: group` | New note ends other voices in the group, whichever instrument they are of | yes |
| `same_note: release` | New note releases the previous voice at the same pitch | yes |
| `max: n`, `steal: oldest` | Caps the voices of the instrument sounding on a part; the oldest end, with a short fade | yes |
| `steal: quietest` | With `max`, ends the quietest voice instead of the oldest | yes |
| `legato: true` (mono) | A note that starts while another is held takes over its voice: envelopes carry on, only the pitch moves | yes |
| `priority: last`, `lowest`, or `highest` (mono, needs `legato`) | Which held note sounds; letting go of one goes back to another still held | yes |
| `glide: 80ms` (mono) | A change of pitch slides to the new note in this time, evenly in semitones | yes |
| `pitch: name` | Which parameter is pitch (default `freq`) | no |
| `compensate: false` | Opt out of latency compensation | no |
| `tail: 2sec` | How long the voice keeps sounding after its release | yes |
| `quantize: 1beat` | When redefinitions land | yes |

An option that isn't built is rejected with a message, not ignored. `mono` is `max: 1`: the previous note fades out over a few milliseconds as the new one starts. Voices are counted per part, so two parts playing the same `mono` instrument each keep their own.

### Playing a mono instrument

```nml
mono bass(freq: hz, amp: db = -6db) with(legato: true, glide: 60ms, priority: lowest) {
  level = env[0 5ms 1 | 200ms 0]
  out = osc(saw, freq).lpf(cutoff: freq * 4).gain(amp) * level
}
```

- **Plain `mono`** retriggers: each note fades the last one out and starts its own envelopes.
- **`glide`** alone makes the new note start from the pitch the last one had reached and slide to its own. It works on the pitch, `freq`, so the instrument needs that parameter.
- **`legato: true`** keeps one voice for as long as notes overlap. A note that starts while another is held doesn't attack again: the envelopes carry on, the note's other parameters (`amp`, lanes) apply, and the pitch moves, with `glide` if there is one. When the newest note ends, the voice goes back to a note still held; when the last one ends, the voice releases. The voice remembers up to 8 held notes.
- **`priority`** picks which held note legato sounds. `last` (the default) is the newest; `lowest` and `highest` hold the lowest or highest pitch, so a higher note played over a held bass note joins it but isn't heard until the low one ends.

With keys from `midi.keys()` this plays like a mono synth: hold a note, roll a second one over it, let go of it, and the first comes back.

**Lifetimes:** a voice ends naturally (released, envelopes and tail done) or forcibly (choked, stolen, replaced), always with a short global fade.

**Parameters are signals.** With `legato: true` a new note updates parameters on the running voice, which is what carries `glide`. A keyboard's pitch bend and pressure set the `bend` and `pressure` parameters of the notes it holds (see [Performance](/reference/language/performance/)). MPE works the same way: `keys: midi.keys(mpe: true)` gives each note its own bend, pressure and a `slide: 0..1` parameter.

**Pitch:** parameter `freq` is pitch by convention — used by `same_note`, mono priority, `keys.note`, and unqualified `transpose`.

No project-wide voice cap in `config` today. Per-instrument limits use `with(max: n, steal: …)` above.

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
