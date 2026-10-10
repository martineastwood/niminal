const test = require("node:test");
const assert = require("node:assert");
const { trackDescription, commands } = require("../panel");

test("a track says what it plays and whether it is muted or soloed", () => {
  assert.strictEqual(trackDescription({ name: "a", clip: null, muted: false, soloed: false }), "stopped");
  assert.strictEqual(trackDescription({ name: "a", clip: "riff", muted: true, soloed: true }), "▶ riff · muted · solo");
  assert.strictEqual(trackDescription({ name: "a", clip: "riff", muted: false, soloed: false, edits: ["reverse", "thin(25%)"] }), "▶ riff · reverse → thin(25%)");
});

test("buttons toggle: a muted track is unmuted", () => {
  assert.strictEqual(commands.mute({ name: "bass", muted: false }), "bass.mute");
  assert.strictEqual(commands.mute({ name: "bass", muted: true }), "bass.unmute");
  assert.strictEqual(commands.solo({ name: "bass", soloed: true }), "bass.unsolo");
  assert.strictEqual(commands.launch("verse"), "verse.start");
  assert.strictEqual(commands.stop({ name: "lead" }), "lead.stop");
});
