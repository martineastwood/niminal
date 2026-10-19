# niminal clients

`Client` in JavaScript speaks the generic daemon events API (port 7401). Clients own their sequencing model; the daemon schedules notes, parameter changes and DSP definitions. It has no clips, scenes, tempo or meter. Clients convert musical positions to absolute sample timestamps, and durations to seconds.

`LiveClient` controls a separate `niminal live` process (port 7400), which owns clips, scenes, lanes, transforms and quantized edits. VS Code starts a private instance of that process. For terminal use, run `niminal daemon`, then `niminal live` in another terminal. Python also provides `Client` for the generic daemon and `LiveClient` for the separate sequencer.

| Path | Language |
| --- | --- |
| [`js/`](js/) | Node (`niminal-client`); the [VS Code extension](../editors/vscode/) depends on it |
| [`python/`](python/) | Python (`pip install -e clients/python`) |

```js
const { Client, LiveClient } = require("niminal-client");
const c = new LiveClient();
await c.connect();
await c.defineClip({ name: "riff", instrument: "pluck", lanes: { notes: "c4 e4" } });
await c.launch("riff", { quantize: "next bar" });
```

To play notes without the sequencer, connect to the engine (`sendEvents`, `cancelEvents` and `clock` are for its port):

```js
const engine = new Client({ port: 7401 });
await engine.connect();
await engine.sendEvents([
  { op: "define", at: "now", source: "instr pluck(freq: hz) { osc(sine, freq) * env[1 | 50ms 0] }\ntrack lead { instrument = pluck }" },
  { op: "note", target: "lead", at: 96000, dur: "250ms", args: { freq: "c4" } },
]);
console.log(await engine.clock());
```

```python
from niminal import LiveClient, ClipDef, Project

c = LiveClient()
c.connect()
c.define_clip(ClipDef(name="riff", instrument="pluck", lanes={"notes": "c4 e4"}))
c.launch("riff", quantize="next bar")

# Offline: write a v2 project for `niminal render orchestra.nml --score piece.nms`
Project(clips=[...], scenes=[...], arrangement={...}).to_json()
```

A minimal independent tracker client lives in [`examples/tracker.js`](examples/tracker.js). It owns its rows, tempo and timing conversion and uses only the generic engine API. Run `npm install --prefix clients/js`, start `niminal daemon`, then run `node clients/examples/tracker.js`.

Protocol version 2 removes engine transport events and musical timestamps. Engine clock subscriptions use the `clock` topic. The live sequencer still exposes its musical `transport` topic.

Protocol version 3 adds private routing graphs and `note.to`. Use `client.routing(edits, { at })` in JavaScript or `client.routing(edits, at=sample)` in Python to edit processors, instances, buses and connections. [`examples/routing.js`](examples/routing.js) demonstrates two independent instances of one effect. Processor definitions and routing IDs are connection-owned; sharing a bus requires `shared: true` and its owner’s `client_id` from `hello`.

Use `transaction` to compile DSP definitions and edit routing as one graph change. Use routing `set` and `gain` edits for lightweight live controls, and `replace` with `fade` for processor crossfades. Subscribe to `transactions` for actual DSP application acknowledgements; `meters` includes public node and bus peaks. See the [Engine API](../docs/src/content/docs/reference/engine-api.md#live-graph-updates).
