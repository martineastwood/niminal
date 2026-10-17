# niminal clients

Thin libraries that speak the daemon's protocols. `niminal daemon` runs two parts: the **live sequencer** on port 7400 (code, clips, scenes, quantized changes) and the **engine** on port 7401, a sound server that plays timestamped events. The clients build clip/scene defs and send them to the sequencer, or send events straight to the engine; the sequencer owns looping and landing on bar lines, and the engine owns time and the session log.

| Path | Language |
| --- | --- |
| [`js/`](js/) | Node (`niminal-client`); the [VS Code extension](../editors/vscode/) depends on it |
| [`python/`](python/) | Python (`pip install -e clients/python`) |

```js
const { Client } = require("niminal-client");
const c = new Client({ port: 7400 });
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
from niminal import Client, ClipDef, Project

c = Client()
c.connect()
c.define_clip(ClipDef(name="riff", instrument="pluck", lanes={"notes": "c4 e4"}))
c.launch("riff", quantize="next bar")

# Offline: write a v2 project for `niminal render orchestra.nml --score piece.nms`
Project(clips=[...], scenes=[...], arrangement={...}).to_json()
```
