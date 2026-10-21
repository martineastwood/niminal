---
title: Routing
description: Buses, execution order, delay, and channel layouts.
---

## Instruments, parts and processors

An instrument is a per-note template. Every note gets its own DSP state. A processor is a persistent audio instance: its state is shared across incoming notes and survives their release. A part chooses an instrument and routes its audio.

```nml
instr pluck(freq: hz) {
  osc(saw, freq) * env[1 | 200ms 0]
}

processor echo {
  out = it.delay(time: 250ms, feedback: 0.4)
}

part lead {
  instrument = pluck
  output = echo
}
```

Here one delay processes the sum of the lead's voices. Putting the delay inside `instr pluck` would create a separate delay for every note. Several parts can output to `echo` to share it. Declare two processors to keep their state independent. A processor declaration creates one running instance; use DSP functions to reuse the processing code. The engine routing API also supports reusable processor definitions and explicit instances.

`it` is a processor's summed incoming audio. A processor can also generate audio, read a bus, or process `input(1)` without a part. Processor output goes to master unless its final `out` is routed with `.to(bus)`.

## Buses and sends

A bus is a summing point, declared once and cleared every block. It owns no effect state. A processor reads the bus and owns the effect:

```nml
bus space: stereo

processor lead_fx {
  out = it.gain(-3db)
  space += out.gain(-14db)
}

processor room {
  out = space.reverb(room: 0.8, damp: 0.4)
}

part lead {
  instrument = pluck
  output = lead_fx
}
```

`space += out` sends a copy while `lead_fx` still outputs to master: a parallel effect send. A part's `output = space` routes its entire output into the bus, without a parallel dry path. A part's `output = lead_fx` routes into a processor directly. Omit `output` for direct master output.

Declaration order does not matter. Musical parts belong to the sequencer. The DSP language uses collection `node` declarations for low-level destinations; they accept only instrument defaults, output routing, gain and pan. DSP belongs in `processor` bodies.

## Execution order

The compiler builds a dependency graph from bus reads and writes and sorts it: all writers to a bus run before readers in the same block. File order is irrelevant; redefinition re-sorts the graph.

**Feedback between buses** has no valid order — the compiler rejects it and names the loop. Break it with **`late`**, which costs one block (32 samples by default):

```nml
processor room {
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

Processing latency declared by DSP processors is propagated through the graph. Faster parallel paths receive exact sample delays before they combine. This includes nested processors, array outputs, instrument voices, processor effects and feed-forward bus sends. Conversion filters in `oversample` participate automatically.

```nml
instr tone(freq: hz) { osc(sine, freq) }
part dry { instrument = tone }
bus wet_audio
processor wet_fx {out = oversample(4) { wet_audio.tanh }}
part wet {  instrument = tone
output = wet_audio}
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

Parts and processors are lowered by the live client into private processor instances and explicit output connections. Other clients can build their own graph with the [Engine API](/reference/engine-api/#processor-instances-buses-and-connections): collection nodes, reusable effects, buses and timed connection edits. DSP-defined sends above remain part of their compiled graph. Musical parts and sequencing stay client-owned.
