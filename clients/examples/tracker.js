// A finite tracker phrase. Rows, tempo and note lengths belong entirely to this client.
// Run with a server listening on port 7401: node clients/examples/tracker.js [BPM]
const { Client } = require("../js");

async function main() {
  const bpm = Number(process.argv[2] ?? 125);
  if (!Number.isFinite(bpm) || bpm <= 0) throw new Error("BPM must be positive");
  const rowsPerBeat = 4;
  const rowSeconds = 60 / bpm / rowsPerBeat;
  const rows = ["c3", null, "c3", "g3", null, "eb3", null, "bb3",
                "c4", null, "g3", null, "eb3", "g3", null, null];
  const engine = new Client({ port: Number(process.env.NIMINAL_ENGINE_PORT ?? 7401) });
  await engine.connect();
  try {
    await engine.sendEvents([{
      op: "define", at: "now",
      source: "instr tracker_tone(freq: hz) { osc(saw, freq) * env[0.15 | 80ms 0] }",
    }]);
    const { sample, sample_rate } = await engine.clock();
    const start = sample + Math.round(sample_rate * 0.2);
    await engine.sendEvents(rows.flatMap((pitch, row) => pitch === null ? [] : [{
      op: "note", target: "tracker_tone",
      at: start + Math.round(row * rowSeconds * sample_rate),
      dur: `${rowSeconds * 0.8}sec`, args: { freq: pitch }, tag: "phrase",
    }]));
    // Keep the connection alive until the phrase and its release have sounded.
    await new Promise(resolve => setTimeout(resolve, (0.4 + rows.length * rowSeconds) * 1000));
  } finally {
    engine.close();
  }
}
main().catch(error => { console.error(error.message); process.exitCode = 1; });
