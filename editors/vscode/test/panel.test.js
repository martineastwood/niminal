const test = require("node:test");
const assert = require("node:assert");
const { partDescription } = require("../panel");

test("a node says what it plays and whether it is muted or soloed", () => {
  assert.strictEqual(partDescription({ name: "a", clip: null, muted: false, soloed: false }), "stopped");
  assert.strictEqual(partDescription({ name: "a", clip: "riff", muted: true, soloed: true }), "▶ riff · muted · solo");
  assert.strictEqual(
    partDescription({ name: "a", clip: "riff", muted: false, soloed: false, edits: ["reverse", "thin(25%)"] }),
    "▶ riff · reverse → thin(25%)",
  );
});
