const { test } = require("node:test");
const assert = require("node:assert/strict");
const { EventEmitter } = require("node:events");
const { startLive } = require("../live");

function child() {
  const process = new EventEmitter();
  process.stdout = new EventEmitter();
  process.stderr = new EventEmitter();
  process.kill = () => { process.killed = true; };
  return process;
}

test("editor owns a private sequencer connected to the generic server", async () => {
  const process = child();
  const live = startLive({ path: "/a path/nimlive", port: 7401, token: "secret" }, () => {}, (path, args, options) => {
    assert.equal(path, "/a path/nimlive");
    assert.deepEqual(args, ["live", "--engine-port", "7401", "--port", "0", "--parent-stdio", "--token", "secret"]);
    assert.equal(options.argv0, "nimlive");
    return process;
  });
  process.stdout.emit("data", "nimlive listening on ws://127.");
  process.stdout.emit("data", "0.0.1:523");
  process.stdout.emit("data", "45\n");
  assert.equal(await live.ready, 52345);
  live.dispose();
  assert.equal(process.killed, true);
});

test("startup failure reaches the editor", async () => {
  const process = child();
  const live = startLive({ path: "niminal", port: 7401 }, () => {}, () => process);
  process.stderr.emit("data", "can't reach the server on port 7401: Connection refused\n");
  process.emit("exit", 1);
  await assert.rejects(live.ready, /exited \(1\): can't reach the server on port 7401/);
});
