---
title: Command line
description: Every niminal subcommand and flag.
---

**Compiled by niminal.** `niminal` is the command line for the engine and for working with `.nml` files without a window: it plays a file and plays it again when it changes, renders a piece to audio, runs the engine as a server, and runs the language server. [Stage](/reference/stage/) is the app for live performance; it has the same engine inside it.

The macOS download puts `niminal` in `/Applications/Niminal/bin` (the Homebrew cask puts it on your PATH). To build it from a clone of the [repo](https://github.com/martineastwood/niminal) (needs a [Rust toolchain](https://rustup.rs/)):

```sh
cargo install --path sequencer/live-cli
```

`niminal <command> --help` lists every flag.

## Subcommands

| Command | What it does |
| --- | --- |
| `niminal play FILE` | Play a file through the sound card, and again each time it, or a module it uses, changes. See [Play](#play). |
| `niminal render` | Render a `.nml` file (or an `arrangement` in it) to WAV; optional `--score`, `--from` / `--to`, `--stems` (one file per node and bus into the `--out` folder). |
| `niminal server` | The audio engine as a server and [events API](/reference/engine-api/) on port 7401 (`--port`), with audio, `--log`, `--token`, `--no-audio`. For testing and for clients of your own. |
| `niminal replay` | Render a session log (what Stage or a server recorded) to WAV. |
| `niminal explain FILE` | Compile a `.nml` file and list, for each instrument and node chain, the ops that run every sample and the ones that run once per block (see [control scopes](/reference/language/units-and-rates/)). |
| `niminal lsp` | The language server on stdio, for editors: see [Editor and LSP](/reference/editor/). |
| `niminal devices` | List the sound cards with the ids `--out-device` and `--in-device` take. |
| `niminal docs` | Emit the built-in opcode reference as Markdown (used when building this site). |

## Play

```sh
niminal play examples/groove.nml
```

Plays the file through your default sound card with the engine and sequencer in this process, and evaluates it again whenever the file or a module it `use`s is saved, so you can edit it in any editor and hear the change. A mistake is shown where it is and the last good version keeps playing. Ctrl+C stops it.

`--out-device`, `--buffer-size`, `--sample-rate` and `--channels` choose how it plays (`--no-audio` plays at real speed with no sound card). `--midi [NAME]` takes MIDI control changes from the inputs whose name contains `NAME` (all of them if empty), `--osc-port PORT` takes OSC over UDP on this machine, and `--clock-in NAME` and `--clock-out NAME` follow or send MIDI clock.

## Render

`--out` is required. Live pieces without a fixed end use `--bars` or `--seconds`. Offline renders can use `--no-limiter`. `niminal render file.nml --score events.nms --out out.wav` plays a [score](/reference/score-model/) against the file's instruments, and `--input take.wav` plays a WAV file into the file's fx, for as long as the file lasts unless you say otherwise with `--bars` or `--seconds`.

Pass the same `--block-size` and `--control-step` the server runs with when you want a render to match what you hear; both are fixed at server start-up and not recorded in session logs.

## `niminal server`

Default listen `127.0.0.1:7401`, stereo output. `--channels` supports 1, 2, 4, 6 (5.1), 8 (7.1). `--sample-rate HZ` defaults to the output device's rate. `--block-size N` sets the samples the server works through at a time (a power of two from 16 to 1024, default 64): control changes and bus sends land on those boundaries, so smaller is tighter and larger is cheaper. It is fixed at start-up, and `late`, bus mappings and the partition size of `convolve` all scale with it; session logs don't record it, so pass the same value to `niminal replay --block-size` and `niminal render --block-size` for identical audio. `--control-step N` (default 64, from 1 to 1024) is the number of samples between the runs of a [`control` scope](/reference/language/units-and-rates/#control-scopes), whatever the block size, like a control period; smaller tracks fast modulation more closely and costs more. It is also fixed at start-up and not recorded in session logs: pass the same value to `niminal replay --control-step` and `niminal render --control-step`. Session logs default to `~/.niminal/sessions/` (newest 20 kept) unless `--no-log` or `--log PATH`. Crashes append to `~/.niminal/crash.log`.

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

The server only plays what it is sent. For the commands that keep time, `niminal play` and `niminal render`, see above. To send notes yourself, see the [engine API](/reference/engine-api/).
