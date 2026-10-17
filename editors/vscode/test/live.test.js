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

test("editor owns a private sequencer connected to the generic daemon", async () => {
  const process = child();
  const live = startLive({ path: "/a path/niminal", port: 7401, token: "secret" }, () => {}, (path, args) => {
    assert.equal(path, "/a path/niminal");
    assert.deepEqual(args, ["live", "--engine-port", "7401", "--port", "0", "--parent-stdio", "--token", "secret"]);
    return process;
  });
  process.stdout.emit("data", "niminal live listening on ws://127.");
  process.stdout.emit("data", "0.0.1:523");
  process.stdout.emit("data", "45\n");
  assert.equal(await live.ready, 52345);
  live.dispose();
  assert.equal(process.killed, true);
});

test("startup failure reaches the editor", async () => {
  const process = child();
  const live = startLive({ path: "niminal", port: 7401 }, () => {}, () => process);
  process.emit("exit", 1);
  await assert.rejects(live.ready, /exited \(1\)/);
});
