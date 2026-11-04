# niminal clients

A small client for the niminal engine (port 7401: `niminal server`, or Stage, which serves it too), in JavaScript. They show how to connect, send timed events and read the clock; they are not a full SDK and will grow once the API settles. Anything else the server offers is reachable with `call(method, params)`, as documented in the [Engine API](../docs/src/content/docs/reference/engine-api.md).

niminal has no clips, scenes, tempo or meter. A client owns its sequencing model, converts musical positions to absolute sample timestamps, and durations to seconds.

The client is in [`js/`](js/): browser ESM and Node (`niminal-client`), the one the VS Code extension is built on.

```js
const { Client } = require("niminal-client");
const engine = new Client({ port: 7401 });
await engine.connect();
await engine.sendEvents([
  { op: "define", at: "now", source: "instr pluck(freq: hz) { osc(sine, freq) * env[1 | 50ms 0] }\nnode lead { instrument = pluck }" },
  { op: "note", target: "lead", at: 96000, dur: "250ms", args: { freq: "c4" } },
]);
console.log(await engine.clock());
```

A minimal step sequencer lives in [`examples/step_sequencer.js`](examples/step_sequencer.js). It owns its rows, tempo and timing conversion and uses only the engine API. Start `niminal server`, then run `node clients/examples/step_sequencer.js` (after `npm install --prefix clients/js`) or `PYTHONPATH=clients/python python3 clients/examples/tracker.py`.

Definitions, controls, samples and buffers belong to the connection that made them, so keep the connection open while the music plays. `panic()` silences your connection.
