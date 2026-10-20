# niminal clients

Two small example clients for the niminal daemon (port 7401), in JavaScript and Python. They show how to connect, send timed events and read the clock; they are not a full SDK and will grow once the API settles. Anything else the daemon offers is reachable with `call(method, params)`, as documented in the [Engine API](../docs/src/content/docs/reference/engine-api.md).

niminal has no clips, scenes, tempo or meter. A client owns its sequencing model, converts musical positions to absolute sample timestamps, and durations to seconds.

| Path | Language |
| --- | --- |
| [`js/`](js/) | Browser ESM and Node (`niminal-client`) |
| [`python/`](python/) | Python (`pip install -e clients/python`) |

```js
const { Client } = require("niminal-client");
const engine = new Client({ port: 7401 });
await engine.connect();
await engine.sendEvents([
  { op: "define", at: "now", source: "instr pluck(freq: hz) { osc(sine, freq) * env[1 | 50ms 0] }\ntrack lead { instrument = pluck }" },
  { op: "note", target: "lead", at: 96000, dur: "250ms", args: { freq: "c4" } },
]);
console.log(await engine.clock());
```

A minimal tracker lives in [`examples/tracker.js`](examples/tracker.js) and [`examples/tracker.py`](examples/tracker.py). It owns its rows, tempo and timing conversion and uses only the engine API. Start `niminal daemon`, then run `node clients/examples/tracker.js` (after `npm install --prefix clients/js`) or `PYTHONPATH=clients/python python3 clients/examples/tracker.py`.

Definitions, controls, samples and buffers belong to the connection that made them, so keep the connection open while the music plays. `panic()` silences your connection.
