// A collection bus feeding two independent instances of one effect definition.
// Run: niminal daemon; node clients/examples/routing.js
const { Client } = require("../js");
const node = id => ({ kind: "node", id });
const bus = id => ({ kind: "bus", id });
const connect = (id, from, to, gain = 1) => ({ op: "connect", id, from, to, gain });

async function main() {
  const engine = new Client({ port: Number(process.env.NIMINAL_ENGINE_PORT ?? 7401) });
  await engine.connect();
  try {
    const { channels } = await engine.status();
    await engine.sendEvents([
      { op: "define", at: "now", source: "instr routing_tone(freq: hz) { osc(sine, freq) * env[0.15 | 100ms 0] }" },
      { op: "routing", at: "now", edits: [
        { op: "processor", name: "echo", source: "(feedback: num = 0.3) { out = it.delay(time: 120ms, feedback: feedback) }" },
        { op: "bus", id: "send", channels },
        { op: "node", id: "voices" },
        { op: "node", id: "short", processor: "echo", args: { feedback: 0.2 } },
        { op: "node", id: "long", processor: "echo", args: { feedback: 0.6 } },
        connect("dry", node("voices"), { kind: "master" }, 0.6),
        connect("send", node("voices"), bus("send"), 0.5),
        connect("short_in", bus("send"), node("short")),
        connect("long_in", bus("send"), node("long")),
        connect("short_out", node("short"), { kind: "master" }, 0.5),
        connect("long_out", node("long"), { kind: "master" }, 0.5),
      ] },
    ]);
    const { sample, sample_rate } = await engine.clock();
    const start = sample + Math.round(sample_rate * 0.2);
    await engine.sendEvents([
      { op: "note", target: "routing_tone", to: "voices", at: start, dur: "150ms", args: { freq: "c4" } },
      { op: "routing", at: start + sample_rate, edits: [{ op: "drain", id: "long" }] },
      // Keep the return connected during its tail, then free the instance explicitly.
      { op: "routing", at: start + sample_rate * 3, edits: [{ op: "free_node", id: "long" }] },
    ]);
    await new Promise(resolve => setTimeout(resolve, 3500));
  } finally { engine.close(); }
}
main().catch(error => { console.error(error.message); process.exitCode = 1; });
