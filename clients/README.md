# niminal clients

`Client` in JavaScript speaks the generic daemon events API (port 7401). Clients own their sequencing model; the daemon schedules notes, parameter changes and DSP definitions. It has no clips or scenes.

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
  { op: "note", target: "lead", at: "4beat", dur: "1/2beat", args: { freq: "c4" } },
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
