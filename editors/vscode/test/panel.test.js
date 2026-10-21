const test = require("node:test");
const assert = require("node:assert");
const { clipDescription } = require("../panel");

test("a node says what it plays and whether it is muted or soloed", () => {
  assert.strictEqual(clipDescription({ name: "a", clip: null, muted: false, soloed: false }), "stopped");
  assert.strictEqual(clipDescription({ name: "a", clip: "riff", muted: true, soloed: true }), "▶ riff · muted · solo");
  assert.strictEqual(
    clipDescription({ name: "a", clip: "riff", muted: false, soloed: false, edits: ["reverse", "thin(25%)"] }),
    "▶ riff · reverse → thin(25%)",
  );
});
