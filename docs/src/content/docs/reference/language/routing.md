---
title: Routing
description: Buses, execution order, delay, and channel layouts.
---

## Buses

Buses are declared once, summed automatically, and cleared every block.

```nml
bus space: stereo

instr pluck(freq: hz) {
  out = osc(saw, freq).lpf(cutoff: 900hz)
  space += out.gain(-14db)
}

track room {
  out = space.reverb(room: 0.8, damp: 0.4)
}
```

- **`bus += x`** sends a copy; original routing is unchanged.
- **`.to(bus)`** routes output to a bus instead of master.
- A track’s **`out`** goes to master unless routed with `.to(bus)`.
- A bus can process what it collects: `bus space: stereo { out = it.reverb(...) }` — `it` is the sum of sends.

Declaration order does not matter; see `examples/room.nml`.

## Execution order

The compiler builds a dependency graph from bus reads and writes and sorts it: all writers to a bus run before readers in the same block. File order is irrelevant; redefinition re-sorts the graph.

**Feedback between buses** has no valid order — the compiler rejects it and names the loop. Break it with **`prev`**, which costs one block (32 samples by default):

```nml
track room {
  out = space.reverb(room: 0.85)
  echo += prev(out).gain(-20db)
}
```

Inside a voice or a function, `prev(x)` is one sample.

## Delay

Use the **`delay`** opcode. Beat times follow tempo. A changing `time` needs **`max:`** to size the line (at most 30 seconds):

```nml
out = sig.delay(time: 3/16beat, feedback: 0.4)
out = sig.delay(time: oscillated, feedback: 0.3, max: 2sec)   // changing time needs max:
```
## Channel layouts

Signals carry a layout in their type: `mono`, `stereo`, `quad`, `surround(5.1)`, `surround(7.1)`, or `channels(N)` (1–16).

- Prefer **`pan(azimuth:, spread:)`** over hard-coded channel counts so the same code works on stereo or surround.
- Master layout: `config { channels: surround(5.1) }` (or `stereo`, `quad`, `surround(7.1)`, `mono`, `channels(N)`). A project has only one `config`; `channels` is the only setting today.
- Convert explicitly with **`.to_layout(stereo)`** (and other layouts) when a send or instrument layout must match a bus.

The daemon’s `--channels` flag must match the layout you intend to hear (1, 2, 4, 6 for 5.1, or 8 for 7.1).
