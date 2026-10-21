---
title: Routing
description: Buses, execution order, delay, and channel layouts.
---

## Instruments and fx instances

An instrument is a per-note template: every note gets its own DSP state. A clip selects the instrument and supplies notes and parameter lanes. An fx is a persistent audio instance: its state is shared across incoming notes and survives their release.

## Buses and sends

A bus sums audio and owns no effect state. Instruments send audio to it, and an fx reads it:

```nml
bus space: stereo

instr pluck(freq: hz) {
  out = (osc(saw, freq) * env[1 | 200ms 0]).pan
  space += out.gain(-14db)
}

fx room {
  out = space.reverb(room: 0.8, damp: 0.4)
}

clip lead {
  instrument: pluck
  notes: [c4 e4 g4]
}
lead.start
```

`space += out` sends a copy while the instrument also outputs to master. For an entirely wet signal, send a local signal to the bus and omit `out` and a final value. Several instruments can send to the same bus to share one fx instance. Declare separate buses and fx instances for independent effect state.

An fx can also generate audio or process `input(1)`. Fx output goes to master unless its final `out` is routed with `.to(bus)`. Declaration order does not matter.

## Execution order

The compiler builds a dependency graph from bus reads and writes and sorts it: all writers to a bus run before readers in the same block. File order is irrelevant; redefinition re-sorts the graph.

**Feedback between buses** has no valid order — the compiler rejects it and names the loop. Break it with **`late`**, which costs one block (32 samples by default):

```nml
fx room {
  out = space.reverb(room: 0.85)
  echo += late(out).gain(-20db)
}
```

Inside a voice or a function, `prev(x)` is one sample. `prev` in a send is an error, so `prev` always means one sample and `late` always means one block.

## Delay

Use the **`delay`** opcode. Beat times follow tempo. A changing `time` needs **`max:`** to size the line (at most 30 seconds):

```nml
out = sig.delay(time: 3/16beat, feedback: 0.4)
out = sig.delay(time: oscillated, feedback: 0.3, max: 2sec)   // changing time needs max:
```
## Latency compensation

Processing latency declared by DSP operations is propagated through the graph. Faster parallel paths receive exact sample delays before they combine. This includes nested effects, array outputs, instrument voices, fx instances and feed-forward bus sends. Conversion filters in `oversample` participate automatically.

```nml
bus wet_audio
instr tone(freq: hz) {
  out = osc(sine, freq)
  wet_audio += out
}
fx wet_fx { out = oversample(4) { wet_audio.tanh } }
clip lead { instrument: tone }
```

The dry path is delayed to match the wet path. The engine `status` result reports `processing_latency` in samples; this excludes the master limiter and audio device latency. Compensation adds causal delay to faster paths; it cannot produce audio before the triggering event.

Musical `delay`, sample feedback with `prev`, and explicit block feedback with `late` keep their intended timing. Feedback routes are not converted into a feed-forward latency budget. Filters' frequency-dependent phase shifts are not processing latency and are not cancelled by compensation.

The timing plan belongs to the compiled topology. Held voices keep their original graphs across live redefinitions, so changing the latency of a topology can leave existing voices on their previous timing until they end.

## Channel layouts

Signals carry a layout in their type: `mono`, `stereo`, `quad`, `surround(5.1)`, `surround(7.1)`, or `channels(N)` (1–16).

- Prefer **`pan(azimuth:, spread:)`** over hard-coded channel counts so the same code works on stereo or surround.
- Master layout: `config { channels: surround(5.1) }` (or `stereo`, `quad`, `surround(7.1)`, `mono`, `channels(N)`). A project has only one `config`; `channels` is the only setting today.
- Convert explicitly with **`.to_layout(stereo)`** (and other layouts) when a send or instrument layout must match a bus.

The server’s `--channels` flag must match the layout you intend to hear (1, 2, 4, 6 for 5.1, or 8 for 7.1).

## Engine routing API

Clips and fx declarations are lowered by the live client into private fx instances and explicit output connections. Other clients can build their own graph with the [Engine API](/reference/engine-api/#fx-instances-buses-and-connections): collection nodes, reusable effects, buses and timed connection edits. DSP-defined sends above remain part of their compiled graph. Clips and sequencing stay client-owned.
