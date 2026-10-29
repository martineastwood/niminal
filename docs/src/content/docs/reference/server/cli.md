---
title: Command line
description: Every niminal subcommand and flag, and what it logs.
---

**Compiled by niminal.** This page is the audio server: the program that makes the sound, and the only one you always need. It knows nothing of tempo, clips or patterns — those belong to a [sequencer](/reference/niminal-live/cli/) or to another [client](/reference/engine-api/).

Install it from a clone of the [repo](https://github.com/martineastwood/niminal) (needs a [Rust toolchain](https://rustup.rs/)):

```sh
cargo install --path crates/niminal-cli
```

`niminal <command> --help` lists every flag.

## Subcommands

| Command | What it does |
| --- | --- |
| `niminal server` | Generic audio server and [events API](/reference/engine-api/) on port 7401 (`--port`), with audio, `--log`, `--token`, `--no-audio`. |
| `niminal replay` | Render a session log (what a server recorded with `--log`, or any client wrote) to WAV. |
| `niminal devices` | List the sound cards with the ids `--out-device` and `--in-device` take. |
| `niminal presets` | List the ready-made instruments and effects. `niminal presets NAME` prints one to paste into a file, `--kind instr` or `--kind fx` narrows the list, and `--json` prints the whole catalog. |
| `niminal docs` | Emit the built-in opcode reference as Markdown (used when building this site). |

## `niminal server`

Default listen `127.0.0.1:7401`, stereo output. `--channels` supports 1, 2, 4, 6 (5.1), 8 (7.1). `--sample-rate HZ` defaults to the output device's rate. `--block-size N` sets the samples the server works through at a time (a power of two from 16 to 1024, default 64): control changes and bus sends land on those boundaries, so smaller is tighter and larger is cheaper. It is fixed at start-up, and `late`, bus mappings and the partition size of `convolve` all scale with it; session logs don't record it, so replay with the same value for identical audio. `--control-step N` (default 64, from 1 to 1024) is the number of samples between the runs of a [`control` scope](/reference/language/units-and-rates/#control-scopes), whatever the block size, like a control period; smaller tracks fast modulation more closely and costs more. It is also fixed at start-up and not recorded in session logs: pass the same value to `niminal replay --control-step` and `niminal-live render --control-step`. Session logs default to `~/.niminal/sessions/` (newest 20 kept) unless `--no-log` or `--log PATH`. Crashes append to `~/.niminal/crash.log`.

| Flag | Default | Meaning |
|---|---|---|
| `--channels N` | 2 | Output channels. |
| `--sample-rate HZ` | device's | Sample rate. |
| `--block-size N` | 64 | Samples per block, as above. |
| `--buffer-size N` | 256, or nearest the device allows | Frames per sound card callback. Smaller is lower latency and less forgiving. |
| `--device NAME` | system default | Sound card for output and input, by part of its name. |
| `--out-device ID` | system default | Output device: an id from `niminal devices` (`output:1`) or part of its name. |
| `--in-device ID` | system default | Input device; needs `--audio-input`. |
| `--audio-input` | off | Listen to the input device, for `input(..)`. |
| `--input-channels N` | all the device has | Keep only the first N input channels; needs `--audio-input`. |
| `--max-voices N` | 512 | Most notes sounding at once; past it the oldest end early. |
| `--render-threads N` | cores, up to 4 | Threads that render together. |
| `--render-fork MODE` | `auto` | When a node's voices are shared among the render threads: `auto`, `always`, `never`, or the microseconds of work a block needs before it is shared. The sound is the same whichever you choose. |
| `--token TOKEN` | none | Require clients to give this token when they say hello. |
| `--no-audio` | off | Run without a sound card, at real speed, throwing the audio away. |
| `--log PATH`, `--no-log` | `~/.niminal/sessions` | Where to record the session, or don't. |
| `--listen ADDR` | `127.0.0.1` | Address to listen on (local only for now). |
| `-v`, `-vv` | off | See below. |

**Logging** — by default the server prints its listening line and any audio trouble. `-v` adds the settings it started with, clients connecting, saying hello and disconnecting, requests that fail, and audio restarts. `-vv` adds every request with how long it took (not the clock polls) and a status line every five seconds: voices, the slowest render, late events, starved frames, deadline misses and underruns. Log lines are queued to a thread of their own, and nothing is written from the audio thread, so verbose logging doesn't affect the sound.

## `niminal replay`

`niminal replay <log> --out set.wav --channels 2` turns a session log into audio, sample for sample. Pass the same `--block-size` and `--control-step` the server ran with, or the render will differ. Pass the server's `--sample-rate` too (the default is 48000, and the log doesn't record it). `--from SECONDS` and `--to SECONDS` keep only that stretch of the session, such as one take of a set.

## What plays it

The server only plays what it is sent. For the commands that keep time — `niminal-live render`, `live`, `repl`, `send` — see [niminal-live's command line](/reference/niminal-live/cli/). To send notes yourself, see the [engine API](/reference/engine-api/).
