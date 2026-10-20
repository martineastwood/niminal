// Run `niminal daemon`, then `node clients/examples/export.js [output.wav]`.
const { writeFile } = require("node:fs/promises");
const { Client } = require("../js");
async function main() {
  const client = new Client({ port: Number(process.env.NIMINAL_ENGINE_PORT ?? 7401) });
  await client.connect();
  try {
    const job = await client.render({
      source: "instr tone(freq: hz) { osc(sine, freq) * env[0.2 | 100ms 0] }",
      events: [{ op: "note", target: "tone", at: 0, dur: "250ms", args: { freq: "440hz" } }],
      frames: 24000, sample_rate: 48000, channels: 2, tail: false,
    });
    let status;
    do {
      await new Promise(resolve => setTimeout(resolve, 20));
      status = await client.jobStatus(job.id);
    } while (status.state === "running");
    if (status.state !== "completed") throw new Error(status.error);
    const file = status.outputs[0].file;
    await writeFile(process.argv[2] ?? "niminal-export.wav", await client.downloadFile(file));
    await client.removeFile(file);
  } finally { client.close(); }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
