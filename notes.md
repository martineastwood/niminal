# Dogfood

```sh
nimlive render examples/synth_drums.nml --out synth_drums.wav
nimlive render examples/saw_lead.nml --out saw_lead.wav
niminal daemon examples/groove.nml
# then: capture last 4bars as jam
```

# Parallel DSP: Phase 0 baseline (2026-10-20)

`cargo test --release -p niminal-daemon --test heavy -- --ignored --nocapture`
(48 kHz stereo, 2 s offline, block = 32 frames = 667 µs budget; 4 saw/square + lpf per voice)

| Piece | µs/block | one core |
| --- | ---: | ---: |
| 1 track, 256 voices | 268 | 40% |
| 8 tracks, 32 voices each | 265 | 40% |
| 4 tracks (32 voices each) + delay/reverb bus | 142 | 21% |

Cost is ~1 µs per voice per block and the bus effects are negligible next to the voices.
Gate result: voice pools dominate, and one track with many voices costs the same as eight
tracks. Track-level parallelism alone would not help the one-instrument case, so the
parallel unit has to be a fixed-size chunk of voices (plan Phase 3), not a whole track.

## Parallel DSP: built (2026-10-20)

Voices are summed in chunks of `VOICE_CHUNK` = 8 (chunk = admission count / 8 within a track, so it
does not depend on when voices end: chunking by list position made live and offline renders differ
by an ulp). Each chunk sums into its own lane and sends into its own `Sends`; lanes merge into the
track and the buses in chunk order. `Fork` (engine trait) runs the chunks of a wave; `Pool`
(niminal-daemon) is the spin-then-park implementation; `--render-threads` on `niminal daemon`.
Tracks with fewer than `FORK_MIN_VOICES` = 16 voices stay on the render thread.

Release, 2 s offline render, serial vs helpers (all output bit-identical to serial):

| Piece | serial | +3 helpers | +7 helpers |
| --- | ---: | ---: | ---: |
| 1 track, 256 voices | 276 | 82 (3.4x) | 68 (4.1x) |
| 8 tracks x 32 voices | 270 | 87 (3.1x) | 214 (1.3x) |
| 4 tracks + delay/reverb bus | 143 | 51 (2.8x) | 108 (1.3x) |

More helpers than chunks to share out makes it slower, hence the default of 4 threads.
Gotchas found on the way: std `Mutex` allocates on first lock on this toolchain, so nothing on the
render path may use one (lanes and slots use `UnsafeCell` handoffs); worker threads need their own
`FlushToZero`; a pool batch must carry its job count in the ticket it is claimed with.
Not done: wasm build of the engine not checked (target not installed here), per-track CPU in
`RenderMetrics`, a fork threshold based on measured cost instead of voice count.

### Parallel DSP, round two (2026-10-20): adaptive threshold, cross-track waves

- Fork decision is by time: every job times itself (`Fork::now`), each track keeps an EMA of ns per
  voice-frame (`Track::rate`, updated only from blocks of >= 16 frames), and `Fork::worth_it(work_ns,
  jobs)` compares the estimated serial work with the pool's measured wake-up cost (measured at
  `Pool::new`, x3 margin). `--render-fork auto|always|never|<micros>`. Tests use `Always`, so the
  parallel path runs even for small pieces. Engine stays clock-free (wasm-safe).
- Waves now take chunks from several tracks: `order` is cut into runs of consecutive independent
  tracks (`independent_runs`); a run's voices render first (per-track `summed`), then each track's
  chain runs serially. Tracks whose voices write a shared buffer (`voice_buffer_writes`) run their
  chunks one at a time, in order (a mutation check showed the test fails without that).
- Release, microseconds per block (serial / auto+3 helpers):
  10 tracks x 10 voices 104 / 41 (2.5x, was 1.0x); 8 x 32 voices 267 / 83 (3.2x; +7 helpers is now
  3.3x, was 1.3x); 16 x 6 sine 43 / 17.5 (2.4x); 2 x 2 sine 1.9 / 2.1 (auto declines, always 2.6);
  6 tracks with heavy chains and 4 voices 48 / 35 (the voices share, the chains don't).
- Pool overhead on this machine: about 0.6 us raw. So Auto and Always rarely differ here; the rule
  matters on machines where waking threads is slower.
- Still open: chains in the wave (they write sends straight to buses), per-track CPU in
  `RenderMetrics`, wasm build of the engine unchecked.
