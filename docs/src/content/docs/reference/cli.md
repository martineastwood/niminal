---
title: Command line
description: The niminal subcommands.
---

There are two programs. `niminal` is the audio server. `nimlive` is a live-coding sequencer that is one client of it; it owns clips, scenes, patterns and tempo. Install from source (needs a [Rust toolchain](https://rustup.rs/)):

```sh
cargo install --path crates/niminal-cli   # niminal
cargo install --path sequencer/nimlive    # nimlive
```

Run `niminal <command> --help` or `nimlive <command> --help` for every flag.

## Subcommands

| Command | What it does |
| --- | --- |
| `niminal server` | Generic audio server and [events API](/reference/engine-api/) on port 7401 (`--port`), with audio, `--log`, `--token`, `--no-audio`. |
| `niminal replay` | Render a session log (what a server recorded with `--log`, or any client wrote) to WAV. |
| `nimlive render` | Render a `.nml` file (or an `arrangement` in it) to WAV; optional `--score`, `--from` / `--to`, `--stems` (one file per node and bus into the `--out` folder). |
| `nimlive live [FILE]` | Separate live sequencer client: clips, scenes, quantized edits, file watch, `--midi`, `--osc-port`. Connects to `--engine-port` (7401); live-control API on `--port` (7400). |
| `nimlive repl` | REPL connected to a running server (`--port`, `--quantize`, `--token`). |
| `nimlive send` | One-shot eval to the server (code argument, `--file`, or stdin). |
| `nimlive explain FILE` | Compile a `.nml` file and list, for each instrument and node chain, the ops that run every sample and the ones that run once per block (see [control scopes](/reference/language/units-and-rates/#control-scopes)). |
| `nimlive lsp` | Language server on stdio — see [Editor and LSP](/reference/editor/). |
| `niminal devices` | List the sound cards with the ids `--out-device` and `--in-device` take. |
| `niminal docs` | Emit built-in opcode reference as Markdown (used when building the docs site). |

## Common flags

**Server** — default listen `127.0.0.1:7401`, stereo output. `--channels` supports 1, 2, 4, 6 (5.1), 8 (7.1). `--sample-rate HZ` defaults to the output device's rate. `--block-size N` sets the samples per DSP block (a power of two from 16 to 1024, default 64): control changes and bus sends land on block boundaries, so smaller blocks are tighter and larger ones cheaper. It is fixed at start-up, and `late`, bus mappings and `convolve` latency all scale with it; session logs don't record it, so replay with the same value for identical audio. `--control-step N` (default 64, from 1 to 1024) is the number of samples between the runs of a [`control` scope](/reference/language/units-and-rates/#control-scopes), whatever the block size, like a control period; smaller tracks fast modulation more closely and costs more. It is also fixed at start-up and not recorded in session logs: pass the same value to `niminal replay --control-step` and `nimlive render --control-step`. `nimlive live [FILE]` loads a project and reloads it on save. Session logs default to `~/.niminal/sessions/` (newest 20 kept) unless `--no-log` or `--log PATH`. Crashes append to `~/.niminal/crash.log`.

**Server audio options**

| Flag | Default | Meaning |
|---|---|---|
| `--channels N` | 2 | Output channels. |
| `--sample-rate HZ` | device's | Sample rate. |
| `--block-size N` | 64 | DSP block size, as above. |
| `--buffer-size N` | 256, or nearest the device allows | Frames per sound card callback. Smaller is lower latency and less forgiving. |
| `--device NAME` | system default | Sound card for output and input, by part of its name. |
| `--out-device ID` | system default | Output device: an id from `niminal devices` (`output:1`) or part of its name. |
| `--in-device ID` | system default | Input device; needs `--audio-input`. |
| `--audio-input` | off | Listen to the input device, for `input(..)`. |
| `--input-channels N` | all the device has | Keep only the first N input channels; needs `--audio-input`. |
| `--max-voices N` | 512 | Most notes sounding at once; past it the oldest end early. |
| `--render-threads N` | cores, up to 4 | Threads that render together. |
| `--listen ADDR` | `127.0.0.1` | Address to listen on (local only for now). |
| `-v`, `-vv` | off | See below. |

**Logging** — by default the server prints its listening line and any audio trouble. `-v` adds the settings it started with, clients connecting, saying hello and disconnecting, requests that fail, and audio restarts. `-vv` adds every request with how long it took (not the clock polls) and a status line every five seconds: voices, the slowest render, late events, starved frames, deadline misses and underruns. Log lines are queued to a thread of their own, and nothing is written from the audio thread, so verbose logging doesn't affect the sound.

**Render** (`nimlive`) — `--out` is required. Live pieces without a fixed end use `--bars` or `--seconds`. Offline renders can use `--no-limiter`. Replay: `niminal replay <log> --out set.wav --channels 2`. Score: `nimlive render file.nml --score events.nms --out out.wav`.

**Send / REPL** — `--quantize` accepts values like `now`, `next bar`, `next 4bars` (same as in source `@ next 4bars`). In the REPL you can also set or clear the default with `:quantize WHEN` or `:quantize off`, and leave with `:quit`.
