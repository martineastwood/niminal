const test = require("node:test");
const assert = require("node:assert");
const net = require("node:net");
const { WebSocketServer } = require("ws");
const { LiveClient: Client } = require("../live-client");

// The live sequencer's protocol is tested against the real sequencer in Rust (and against Stage, over a socket,
// in stage/src-tauri). This tests the client: what it sends, and what it makes of the answers, against a server
// that speaks as much of the protocol as the client uses.
function freePort() {
  return new Promise((resolve) => {
    const server = net.createServer().listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

async function withStage(run) {
  const port = await freePort();
  const state = { clips: [], scenes: [], sent: [] };
  const server = new WebSocketServer({ port, host: "127.0.0.1" });
  const timers = [];
  server.on("connection", (socket) => {
    const topics = new Set();
    timers.push(setInterval(() => {
      if (topics.has("transport")) socket.send(JSON.stringify({ jsonrpc: "2.0", method: "transport", params: { bar: 1, beat: 1, bpm: 100, voices: 0, pending: 0 } }));
    }, 50));
    socket.on("message", (data) => {
      const { id, method, params } = JSON.parse(data.toString());
      state.sent.push([method, params]);
      const reply = (result) => id !== undefined && socket.send(JSON.stringify({ jsonrpc: "2.0", id, result }));
      const fail = (message, problems) => socket.send(JSON.stringify({ jsonrpc: "2.0", id, error: { code: -32001, message, data: { problems } } }));
      if (method === "hello") return reply({ protocol: 5, release: null, sample_rate: 48000, channels: 2, topics: ["transport"] });
      if (method === "subscribe") {
        params.topics.forEach((t) => topics.add(t));
        return reply({ topics: [...topics] });
      }
      if (method === "eval") {
        if (params.source.includes("zz")) return fail("unknown note `zz`", [{ line: 2, column: 14, message: "unknown note `zz`" }]);
        for (const m of params.source.matchAll(/^clip (\w+)/gm)) state.clips.push({ name: m[1], muted: false });
        for (const m of params.source.matchAll(/^scene (\w+)/gm)) state.scenes.push({ name: m[1] });
        return reply({ id: 1, lands_at: 0, changes: [] });
      }
      if (method === "launch") return reply({ id: 2, lands_at: 0 });
      if (method === "mute") {
        const clip = state.clips.find((c) => c.name === params.target);
        if (clip) clip.muted = true;
        return reply({ id: 3, lands_at: 0 });
      }
      if (method === "status") return reply({ clips: state.clips, scenes: state.scenes });
      if (method === "hush") return reply({ id: 4, lands_at: 0 });
      fail(`no method \`${method}\``);
    });
  });
  try {
    await run(port, state);
  } finally {
    timers.forEach(clearInterval);
    server.clients.forEach((c) => c.terminate());
    await new Promise((resolve) => server.close(resolve));
  }
}

const setup = `tempo 100bpm
instr tone(freq: hz) { osc(sine, freq) }
clip t { instrument: tone
  notes: [c4] }`;

test("evaluates code, reports problems with positions, and hears transport", async () => {
  await withStage(async (port, state) => {
    const heard = [];
    const client = new Client({ port, client: "niminal-vscode-test", onNotification: (m, p) => heard.push([m, p]) });
    const hello = await client.connect();
    assert.strictEqual(hello.protocol, 5);
    await client.subscribe(["transport"]);

    const ok = await client.eval(setup, { quantize: "now" });
    assert.ok(ok.id !== undefined);
    assert.deepStrictEqual(state.sent.find(([m]) => m === "eval")[1], { source: setup, quantize: "now" });

    await assert.rejects(client.eval("clip bad { instrument: tone\n  notes: [c4 zz] }"), (e) => {
      assert.strictEqual(e.problems[0].line, 2);
      assert.strictEqual(e.problems[0].column, 14);
      assert.match(e.message, /zz/);
      return true;
    });

    await client.hush();
    await new Promise((r) => setTimeout(r, 200));
    assert.ok(heard.some(([m]) => m === "transport"));
    client.close();
  });
});

test("typed launch and mute go through the performance API", async () => {
  await withStage(async (port, state) => {
    const client = new Client({ port, client: "niminal-vscode-test" });
    await client.connect();
    await client.eval(`${setup}\nscene verse { t }\n`, { quantize: "now" });
    const launched = await client.launch("verse", { quantize: "now" });
    assert.ok(launched.id !== undefined);
    await client.mute("t", { quantize: "now" });
    assert.deepStrictEqual(state.sent.filter(([m]) => m === "launch" || m === "mute"), [
      ["launch", { target: "verse", quantize: "now" }],
      ["mute", { target: "t", quantize: "now" }],
    ]);
    const status = await client.status();
    assert.strictEqual(status.clips.find((t) => t.name === "t").muted, true);
    client.close();
  });
});

test("connecting to nothing fails with a clear message", async () => {
  const port = await freePort();
  await assert.rejects(new Client({ port }).connect(), /can't reach a niminal server/);
});
