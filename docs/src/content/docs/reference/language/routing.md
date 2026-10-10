---
title: Routing
description: Buses, execution order, delay lines, latency, and channel layouts.
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
  out = space.reverb(room: 0.8, model: plate)
}
```

- **`bus += x`** sends a copy; original routing is unchanged.
- **`.to(bus)`** routes output to a bus instead of master.
- A track’s **`out`** goes to master unless routed with `.to(bus)`.

## Execution order

The compiler builds a dependency graph from bus reads and writes and sorts it: all writers to a bus run before readers in the same block. File order is irrelevant; redefinition re-sorts the graph.

**Feedback between buses** has no valid order — the compiler rejects it and names the loop. Break it with **`prev`**, which costs one block (32 samples by default):

```nml
track room {
  out = space.reverb(room: 0.85)
  echo += prev(out).gain(-20db)
}
```

Inside a single voice or opcode, `prev(x)` is one sample.

## Delay lines

- **Opcode:** `sig.delay(time: 3/16 beat, feedback: 0.4)` — beat times follow tempo.
- **Delay lines as values** (write with `+=`, read with taps):

```nml
line = delay_line(max: 2sec)
tap1 = line.tap(3/16 beat)
tap2 = line.tap(5/16 beat, interp: cubic)

line += sig + tap1.lpf(cutoff: 3khz).gain(-6db)
out = sig + [tap1, tap2]
```

Buffer size is known up front (`max:` when time is modulated). Short feedback loops (flangers, Karplus–Strong) run per sample automatically. When modulating read position, set **`interp:`** `none`, `linear`, `cubic`, or `allpass`.

## Latency compensation

Opcodes with lookahead declare latency; parallel paths are delayed to stay aligned. Automatic unless `with(compensate: false)`.

```nml
out = drums.split(dry: it, wet: it.limit(ceiling: -1db, lookahead: 5ms)).sum
```

## Channel layouts

Signals carry a layout in their type: `mono`, `stereo`, `quad`, `surround(5.1)`, `surround(7.1.4)`, `ambi(order: 3)`, `ch[N]`.

- **`split`** must cover every channel or provide `rest:`.
- **`.each(...)`** processes every channel of any layout.
- Prefer **`pan(azimuth:, spread:)`** over hard-coded channel names so the same code works on stereo, 5.1, or headphones.
- Master layout: `config { channels: surround(5.1) }`.
- Ambisonics: `config { mix: ambi(order: 3) }` — buses carry the sound field; decoded at master.

Mismatch overrides: `config { upmix: ... }` or `.to_layout(...)`.
