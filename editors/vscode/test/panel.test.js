const test = require("node:test");
const assert = require("node:assert");
const { clipDescription, sceneDescription, silencers } = require("../panel");

test("a clip says whether it plays, what has been done to it, and whether it is muted or soloed", () => {
  assert.strictEqual(clipDescription({ name: "a", running: false, muted: false, soloed: false }), "stopped");
  assert.strictEqual(clipDescription({ name: "a", running: true, muted: true, soloed: true }), "▶ · muted · solo");
  assert.strictEqual(
    clipDescription({ name: "a", running: true, muted: false, soloed: false, edits: ["reverse", "thin(25%)"] }),
    "▶ · reverse → thin(25%)",
  );
});

test("a scene says how many of its clips are playing", () => {
  const scene = { name: "verse", clips: ["bass", "lead", "drums"], running: 0, active: false };
  assert.strictEqual(sceneDescription(scene, "⌘1"), "⌘1");
  assert.strictEqual(sceneDescription({ ...scene, running: 2 }, "⌘1"), "⌘1 · 2/3 playing");
  assert.strictEqual(sceneDescription({ ...scene, running: 3, active: true }, ""), "▶ all playing");
});

test("solo and mute are called out, since they silence clips that are playing", () => {
  assert.strictEqual(silencers([{ name: "a" }, { name: "b" }]), "");
  assert.strictEqual(silencers([{ name: "a", soloed: true }, { name: "b", muted: true }, { name: "c", muted: true }]), "SOLO a · 2 muted");
});
