# niminal clients

Thin libraries that speak the daemon protocol. They build clip/scene defs and send them; the daemon owns timing, looping and the session log.

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

```python
from niminal import Client, ClipDef, Project

c = Client()
c.connect()
c.define_clip(ClipDef(name="riff", instrument="pluck", lanes={"notes": "c4 e4"}))
c.launch("riff", quantize="next bar")

# Offline: write a v2 project for `niminal render orchestra.nml --score piece.nms`
Project(clips=[...], scenes=[...], arrangement={...}).to_json()
```
