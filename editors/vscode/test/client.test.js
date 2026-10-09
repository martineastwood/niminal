const test = require("node:test");
const assert = require("node:assert");
const net = require("node:net");
const { spawn } = require("node:child_process");
const path = require("node:path");
const { Client } = require("../client");

const binary = process.env.NIMINAL_BIN ?? path.join(__dirname, "../../../target/debug/niminal");

function freePort() {
  return new Promise((resolve) => {
    const server = net.createServer().listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

async function withDaemon(run) {
  const port = await freePort();
  const daemon = spawn(binary, ["daemon", "--no-audio", "--port", String(port)]);
  await new Promise((resolve) => daemon.stdout.once("data", resolve));
  try {
    await run(port);
  } finally {
    daemon.kill();
  }
}

const setup = `tempo 100bpm
instr tone(freq: hz) { osc(sine, freq) }
track t { instrument = tone }`;

test("evaluates code, reports problems with positions, and hears transport", async () => {
  await withDaemon(async (port) => {
    const heard = [];
    const client = new Client({ port, onNotification: (m, p) => heard.push([m, p]) });
    await client.connect();
    await client.call("subscribe", { topics: ["transport"] });

    const ok = await client.call("eval", { source: setup, quantize: "now" });
    assert.ok(ok.id !== undefined);

    await assert.rejects(client.call("eval", { source: "play t = [c4 zz]" }), (e) => {
      assert.strictEqual(e.problems[0].line, 1);
      assert.strictEqual(e.problems[0].column, 14);
      assert.match(e.message, /zz/);
      return true;
    });

    await client.call("hush");
    await new Promise((r) => setTimeout(r, 400));
    assert.ok(heard.some(([m]) => m === "transport"));
    client.close();
  });
});

test("connecting to nothing fails with a clear message", async () => {
  const port = await freePort();
  await assert.rejects(new Client({ port }).connect(), /can't reach a niminal daemon/);
});
