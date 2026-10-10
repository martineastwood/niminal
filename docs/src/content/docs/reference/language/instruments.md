---
title: Instruments
description: instr and mono, voices, out, parameters, and multichannel.
---

Instruments are templates; each note is a **voice**. An instrument declares typed, named parameters with defaults.

## Result: last expression or `out`

The result is the **last expression**, or **`out`** if the body uses `out`: `out` starts as silence and can be assigned or added to. Using both in one body is an error. The same rule applies to `opcode` and `fn`.

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
- Calling an instrument inside another gives its audio as a sub-voice.
- A voice lives until its envelopes finish, even past its scheduled duration.
- The compiler warns if an instrument never writes to `out` or a bus.

## `instr` and `mono`

| Keyword | A new note… | Used by |
| --- | --- | --- |
| `instr` | Spawns a new voice (poly, default) | Most instruments |
| `mono` | Updates the one running voice | Bass, leads |

Continuous processors and generators are **tracks** (see [Performance](/reference/language/performance/)), not instruments.

### `with(...)` options

| Option | Effect |
| --- | --- |
| `choke: group` | New note ends other voices in the group |
| `same_note: release` | New note releases the previous voice at the same pitch |
| `max: n, steal: oldest` or `quietest` | Caps voices |
| `legato: true` (mono) | Overlapping notes don’t retrigger envelopes |
| `priority: last`, `lowest`, or `highest` (mono) | Which held note sounds |
| `pitch: name` | Which parameter is pitch (default `freq`) |
| `compensate: false` | Opt out of latency compensation |
| `quantize: 1 beat` | When redefinitions land |

**Lifetimes:** a voice ends naturally (released, envelopes done) or forcibly (choked, stolen, replaced), always with a short global fade.

**Parameters are signals.** In `mono`, a new note updates parameters on the running voice — glide is ordinary processing: `osc(saw, freq.glide(40ms))`. Same mechanism for MPE and aftertouch.

**Pitch:** parameter `freq` is pitch by convention — used by `same_note`, mono priority, `keys.note`, and unqualified `transpose`.

No voice limits by default. `config { max_voices: 512 }` steals quietest voices and warns.

## What you pass is what you get

| You pass | Behavior | Example |
| --- | --- | --- |
| Constant | Stays constant | `amp: -6db` |
| Pattern | Each note gets its own value, fixed at note start | `amp: [-6db -12db -9db]` |
| Signal | Stays live; running notes follow it | `amp: lfo(2 beats).range(-12db..0db)` |

## Multichannel and branching

1. **Multichannel expansion:** comma list in an argument: `src.lpf(cutoff: [800hz, 1200hz])`.
2. **`split` with `it`** for asymmetric processing; `it` alone is pass-through.
3. **Destructuring** for anything more complex.

```nml
out = src.split(
  left:  it.lpf(cutoff: 800hz),
  right: it.delay(time: 3/16 beat).hpf(cutoff: 200hz),
)

out = drums.split(dry: it, wet: it.compress(ratio: 8, threshold: -30db)).sum

[l, r] = src
mid  = (l + r) * 0.5
side = ((l - r) * 0.5).gain(+3db)
out  = [mid + side, mid - side]
```

A comma list of signals builds multichannel audio. The compiler warns when layout doesn’t match the destination.
