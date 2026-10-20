---
title: Samples
description: WAV samples, kits, slices, and playing them from patterns.
---

## Samples and kits

| Declaration | What it is |
| --- | --- |
| `sample(...)` | A playable WAV file, or a table |
| `kit(...)` | A folder of WAVs by name |
| `.slices(n)` | The slices of a sample, as a kit |

```nml
kick = sample("drums/kick.wav")
amen = sample("breaks/amen.wav").beats(4).fit(rate)
drums = kit("drums/808")
chops = amen.slices(16)                // s0 .. s15
```

Paths are resolved relative to the file that declares them (and the project when the server evaluates).

### Settings on a sample

A sample is a plain binding, and its settings are chained methods: `amen = sample("amen.wav").beats(4).fit(rate)`.

| Method | Effect |
| --- | --- |
| `.duration(2sec)` | Physical target length for `.fit(rate)`; also available directly in DSP definitions |
| `.beats(N)` | The loop is N beats long (needed for tempo sync) |
| `.fit(rate)` | Speed the loop up or down so those beats match the current tempo |
| `.root(c4)` | The note (or frequency) at which the sample plays as recorded |
| `.loop(forward)` | Loop the sample while a note is held |
| `.choke(group)` | On a kit sample: starting this sample ends others in the group |

`.fit(stretch)` and `.fit(slice)` are not supported yet. Only `.fit(rate)` is.

### Playing samples

A sample or kit is an instrument. Call it with playback settings, or put it on a track and play note/sample patterns:

```nml
kick(pitch: -2st)
snare(pitch: +2st)
track drums { instrument = chops }
drums.play([s0 s1 s2 s3 s4 s5 s6 s7])
```

A kit member is an instrument too: `drums.snare` means the kit with that sample chosen (the same as `drums(sample: snare)`). Give it its own track when you want effects, sends, mute or solo on that drum alone:

```nml
drums = kit("drums")
track snare {
  instrument = drums.snare
  out = it.reverb(room: 0.4).gain(-3db)
}
snare.play(grid[.... x... .... x...])
```

In patterns, `drums.kick` names the sample when a bare name would clash with a note (`a`, `e`, `c4`).

Parameters you can set on a hit:

| Parameter | Meaning |
| --- | --- |
| `pitch:` or `rate:` or `freq:` | Playback pitch / speed (not more than one of these) |
| `start:` | Start offset into the file |
| `gain:` | Level (as for any instrument) |

Chop a break with a grid — a row per slice, columns are steps. `X` accents, `o` ghosts. A row can take note controls, use mini-notation, or name a drum track so its hits go through that strip:

```nml
drums.play(grid {
  s0:  x... .... x... ....
  s4:  .... x... .... x..x
  s2:  ..x. ..o. ..X. ....
})

beat.play(grid {
  kick:              x... x... x... x..x
  snare(pan: 10deg): [~ x ~ [x o]]
  hat(gain: -6db):   [x*2 x x(3,8)]
})
```

See `examples/drums.nml` and `examples/breakbeat.nml`.

### Choke groups

On a kit, name groups so one hit cuts off another (open/closed hats, overlapping slices):

```nml
drums = kit("drums").choke(hats(hat_open, hat_closed))
chops = brk.slices(16).choke(g(s4, s12))
```

### Tables and samples

A sample's audio is a table. `kick.data` is that table (or an array of channel tables). A const table can be played as a sample: `pad = sample(organ).root(c4).loop(forward)`. See [Tables](/reference/language/tables/).

## Built-in DSP

For filters, delay, reverb, noise, compress, chorus, pan and math on any signal (including sample output), see [Built-in opcodes](/reference/opcodes/). Spectral processing and SFZ multisamples are not in the language yet.
