const test = require("node:test");
const assert = require("node:assert");
const net = require("node:net");
const { spawn } = require("node:child_process");
const path = require("node:path");
const { LiveClient: Client } = require("../live-client");

const { startLive } = require("../live");

const binary = process.env.NIMINAL_BIN ?? path.join(__dirname, "../../../target/debug/niminal");
const nimlive = process.env.NIMLIVE_BIN ?? path.join(__dirname, "../../../sequencer/target/debug/nimlive");

function freePort() {
  return new Promise((resolve) => {
    const server = net.createServer().listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

async function withServer(run) {
  const port = await freePort();
  const server = spawn(binary, ["server", "--no-audio", "--port", String(port)]);
  await new Promise((resolve) => server.stdout.once("data", resolve));
  const live = startLive({ path: nimlive, port }, () => {});
  try {
    await run(await live.ready);
  } finally {
    live.dispose();
    server.kill();
  }
}

const setup = `tempo 100bpm
instr tone(freq: hz) { osc(sine, freq) }
part t { instrument = tone }`;

test("evaluates code, reports problems with positions, and hears transport", async () => {
  await withServer(async (port) => {
    const heard = [];
    const client = new Client({ port, client: "niminal-vscode-test", onNotification: (m, p) => heard.push([m, p]) });
    await client.connect();
    await client.subscribe(["transport"]);

    const ok = await client.eval(setup, { quantize: "now" });
    assert.ok(ok.id !== undefined);

    await assert.rejects(client.eval("t.play([c4 zz])"), (e) => {
      assert.strictEqual(e.problems[0].line, 1);
      assert.strictEqual(e.problems[0].column, 12);
      assert.match(e.message, /zz/);
      return true;
    });

    await client.hush();
    await new Promise((r) => setTimeout(r, 400));
    assert.ok(heard.some(([m]) => m === "transport"));
    client.close();
  });
});

test("typed launch and mute go through the performance API", async () => {
  await withServer(async (port) => {
    const client = new Client({ port, client: "niminal-vscode-test" });
    await client.connect();
    await client.eval(
      `${setup}
clip riff { notes: [c4] }
scene verse { t: riff }
`,
      { quantize: "now" },
    );
    const launched = await client.launch("verse", { quantize: "now" });
    assert.ok(launched.id !== undefined);
    await client.mute("t", { quantize: "now" });
    const status = await client.status();
    const node = status.parts.find((t) => t.name === "t");
    assert.ok(node);
    assert.strictEqual(node.muted, true);
    client.close();
  });
});

test("connecting to nothing fails with a clear message", async () => {
  const port = await freePort();
  await assert.rejects(new Client({ port }).connect(), /can't reach a niminal server/);
});

test("server stays generic while two editor sequencers keep separate scenes", async () => {
  const { Client: EngineClient } = require("niminal-client");
  const port = await freePort();
  const server = spawn(binary, ["server", "--no-audio", "--no-log", "--port", String(port)]);
  await new Promise((resolve) => server.stdout.once("data", resolve));
  const editors = [startLive({ path: nimlive, port }, () => {}), startLive({ path: nimlive, port }, () => {})];
  const engine = new EngineClient({ port });
  const clients = [];
  try {
    await engine.connect();
    await assert.rejects(engine.call("clip.define", { name: "riff" }), /method|unknown/i);
    await engine.sendEvents([{ op: "define", at: "now", source: "instr tone(freq: hz) { osc(sine, freq) }" }]);
    for (const editor of editors) {
      const client = new Client({ port: await editor.ready });
      clients.push(client);
      await client.connect();
    }
    await clients[0].eval(`${setup}\nclip riff { notes: [c4] }\nscene verse { t: riff }`, { quantize: "now" });
    assert.deepEqual((await clients[0].status()).scenes, ["verse"]);
    assert.deepEqual((await clients[1].status()).scenes, []);
    assert.ok((await engine.clock()).sample >= 0);
  } finally {
    clients.forEach((client) => client.close());
    engine.close();
    editors.forEach((editor) => editor.dispose());
    server.kill();
  }
});

test("live sequencer exits when its owning editor closes stdin", async () => {
  const port = await freePort();
  const server = spawn(binary, ["server", "--no-audio", "--no-log", "--port", String(port)]);
  await new Promise((resolve) => server.stdout.once("data", resolve));
  const live = spawn(nimlive, ["live", "--engine-port", String(port), "--port", "0", "--parent-stdio"]);
  try {
    await new Promise((resolve, reject) => {
      live.stdout.once("data", resolve);
      live.once("error", reject);
      live.once("exit", (code) => reject(new Error(`early exit: ${code}`)));
    });
    const exited = new Promise((resolve) => live.once("exit", resolve));
    live.stdin.end();
    const timeout = setTimeout(() => live.kill(), 2000);
    try {
      assert.equal(await exited, 0);
    } finally {
      clearTimeout(timeout);
    }
  } finally {
    live.kill();
    server.kill();
  }
});
