const test = require("node:test");
const assert = require("node:assert");
const { trackDescription, commands } = require("../panel");

test("a track says what it plays and whether it is muted or soloed", () => {
  assert.strictEqual(trackDescription({ name: "a", clip: null, muted: false, soloed: false }), "stopped");
  assert.strictEqual(trackDescription({ name: "a", clip: "riff", muted: true, soloed: true }), "▶ riff · muted · solo");
});

test("buttons toggle: a muted track is unmuted", () => {
  assert.strictEqual(commands.mute({ name: "bass", muted: false }), "mute bass");
  assert.strictEqual(commands.mute({ name: "bass", muted: true }), "unmute bass");
  assert.strictEqual(commands.solo({ name: "bass", soloed: true }), "unsolo bass");
  assert.strictEqual(commands.launch("verse"), "launch verse");
  assert.strictEqual(commands.stop({ name: "lead" }), "stop lead");
});
