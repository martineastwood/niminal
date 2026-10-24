---
title: Samples
description: WAV samples, kits, slices, and playing them from patterns.
---

**Compiled by niminal.** Samples and kits are loaded and played by the server.

## Samples and kits

| Declaration | What it is |
| --- | --- |
| `sample(...)` | A playable WAV file, or a table |
| `kit(...)` | A folder of WAVs by name |
| `.slices(n)` | The slices of a sample, as a kit: `n` equal pieces, or `.slices([0, 0.18, 0.43])` for pieces that start at those shares of its length (0 up to 1, each later than the one before) and run to the next |

```nml
kick = sample("drums/kick.wav")
amen = sample("breaks/amen.wav").beats(4)
drums = kit("drums/808")
chops = amen.slices(16)                // s0 .. s15
hits = amen.slices([0, 0.18, 0.43])   // s0 .. s2, starting at those shares of the sample
```

Paths are resolved relative to the file that declares them (and the project when the server evaluates).

### Settings on a sample

A sample is a plain binding, and its settings are chained methods: `amen = sample("amen.wav").beats(4)`.

| Method | Effect |
| --- | --- |
| `.beats(N)` | The loop is N beats long: it is sped up or down to last N beats at the current tempo |
| `.duration(2sec)` | The same, to a fixed length that ignores the tempo; also available directly in DSP definitions |
| `.root(c4)` | The note (or frequency) at which the sample plays as recorded |
| `.interp(sinc)` | Rate-aware windowed sinc playback; `.interp(cubic)` is the faster default |
| `.loop(forward)` | Sustain a loop while a note is held; `.loop(bidirectional)` alternates direction |
| `.choke(group)` | On a kit sample: starting this sample ends others in the group |

Use `pad = sample("pad.wav").interp(sinc)` for higher-quality pitch and sample-rate conversion. The same setting works on kits and slices, such as `chops = amen.slices(16).interp(sinc)`. Sinc filtering costs more CPU and reduces aliasing when playing faster. Its filter is limited to 512 taps, so suppression decreases at extreme playback rates.

For a sustain region inside a whole sample, call a looped sampler with `loop_start` and `loop_end` as shares of its length:

```nml
pad = sample("pad.wav").root(c4).loop(forward)
instr held(freq: hz) {
  pad(freq: freq, loop_start: 0.2, loop_end: 0.8) * env[1 | 20ms 0]
}
```

The intro plays once, then frames from Start (inclusive) to End (exclusive) repeat. Reverse playback plays the outro first, then repeats the same region backwards. Points are captured at note start and rounded to source frames; invalid or empty regions use the whole sample. Bounds default to 0 and 1. On slices, the bounds are relative to the piece; the tracker's loop editor is for whole samples. Loop points do not automatically crossfade, so choose matching waveform positions to avoid clicks.

Fitting changes the playback speed (and so the pitch). Use `amen(stretch: 2)` to double the playback duration without changing the selected pitch, or `stretch: 0.5` to halve it. The duration multiplier ranges from 0.25 to 4 and works with samples, kits, slices, reverse playback and loops. The tracker exposes the duration as **Stretch ×** and the algorithm as **Granular / WSOLA** in the Sample panel. Both algorithms use overlapping 50 ms grains:

| `stretch_mode` | Algorithm | Tradeoff |
| --- | --- | --- |
| `0` (default) | Granular | Fixed grain positions; lower CPU cost, with a more audible granular texture |
| `1` | WSOLA | Searches nearby for matching waveforms before joining grains, reducing phase cancellation on suitable material; uses more CPU |
| `2` | Transient WSOLA | Detects attacks offline, plays short attack regions at their native pitch speed, and stretches the bodies between them |

```nml
instr slowed(freq: hz) {
  amen(freq: freq, stretch: 1.5, stretch_mode: 1)
}
```

WSOLA searches up to 10 ms of source audio on either side of the expected grain position (capped at 2048 frames). All channels use the same analysis channel, chosen by its energy, so alignment preserves stereo timing even with a silent channel or opposite-polarity channels. Ordinary WSOLA does not protect attacks, so drums can still smear or repeat. **Transient WSOLA** uses the same amplitude-onset detector as Find hits. It protects up to 15 ms after each detected onset and compensates in the body so onset timing and overall duration stay fitted. Protection is shortened for short segments or extreme compression. Grain reads and interpolation stay out of neighbouring attack regions. Reverse playback (including the reverse leg of ping-pong loops) uses ordinary WSOLA. Detection can miss quiet or closely spaced attacks, and large stretch ratios can still sound unnatural. At `stretch: 1`, both modes use ordinary sample playback. Algorithm changes take effect when the next grain starts.

Slice-fitting is not supported yet.

### Playing samples

A sample or kit is an instrument. Call it with playback settings, or select it in a clip and play note/sample patterns:

```nml
kick(pitch: -2st)
snare(pitch: +2st)
clip chop {
  instrument: chops
  notes: [s0 s1 s2 s3 s4 s5 s6 s7]
}
chop.start
```

A kit member is an instrument too: `drums.snare` means the kit with that sample chosen (the same as `drums(sample: snare)`). Give it its own clip when you want effects, sends, mute or solo on that drum alone:

```nml
drums = kit("drums")
bus snare_audio
fx snare_fx {out = snare_audio.reverb(room: 0.4).gain(-3db)}
instr snare_voice(freq: hz) {
  voice = drums.snare(freq: freq)
  snare_audio += voice
}
clip snare {
  instrument: snare_voice
  notes: [~ c4 ~ c4]
}
snare.start
```

### Calling samplers inside an instrument

A sample, a kit member, or a whole kit can produce audio inside an `instr`:

```nml
instr snare_voice(freq: hz) {
  out = drums.snare(freq: freq)
  room += out.gain(-12db)
}

instr drum_voice(sample) {
  out = drums(sample: sample)
  room += out.gain(-12db)
}

hits = grid {
  kick:  x... x...
  snare: .... x...
  hat:   x.x. x.x.
}
clip beat { instrument: drum_voice, notes: hits }
beat.start
```

The `sample` parameter selects a kit member for each hit. Forward it as `sample: sample` to keep the kit's member names and choke groups. One wrapper can route the entire kit; separate wrappers are useful for different effects or sends per drum. A wrapper can also declare parameters such as `freq: hz`, `pitch: st`, or `gain: db` and pass them to the sampler. Playback settings omitted from the call use the sampler's defaults, including its root pitch, fitted duration, slices and looping. Voice policies belong to the wrapper instrument and count per clip name.

Only instruments can call samplers. For an entirely wet route, send the sampler signal to a bus and omit `out` and a final value. The fx reading the bus keeps its effect state across notes.

In patterns, `drums.kick` names the sample when a bare name would clash with a note (`a`, `e`, `c4`).

Parameters you can set on a hit:

| Parameter | Meaning |
| --- | --- |
| `pitch:` or `rate:` or `freq:` | Playback pitch / speed (not more than one of these) |
| `start:` | Start offset into the file |
| `begin:` | Start offset as a share of the sample's (or slice's) length, 0 to 1 |
| `gain:` | Level (as for any instrument) |

A negative `rate:` when the note starts plays the sample backwards from its end (`rate: -1` is the sample reversed at its recorded speed), and `start:` and `begin:` then count back from the end. The direction is fixed as the note starts; the speed can still change afterwards.

Chop a break with a grid — a row per slice, columns are steps. `X` accents, `o` ghosts. A row can take note controls, use mini-notation, or name a drum node so its hits go through that strip:

```nml
clip chop {
  instrument: chops
  notes: grid {
    s0:  x... .... x... ....
    s4:  .... x... .... x..x
    s2:  ..x. ..o. ..X. ....
  }
}

clip beat {
  instrument: drum_voice
  notes: grid {
    kick:              x... x... x... x..x
    snare(pan: 10deg): [~ x ~ [x o]]
    hat(gain: -6db):   [x*2 x x(3,8)]
  }
}
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
