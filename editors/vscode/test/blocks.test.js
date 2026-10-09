const test = require("node:test");
const assert = require("node:assert");
const { blockAround } = require("../blocks");

const lines = (text) => text.split("\n");

test("a block is the lines around the cursor up to blank lines", () => {
  const src = lines("tempo 100bpm\n\nplay a = [c4]\nplay b = [e4]\n\nhush");
  assert.deepStrictEqual(blockAround(src, 3), { start: 2, end: 3 });
  assert.deepStrictEqual(blockAround(src, 0), { start: 0, end: 0 });
  assert.strictEqual(blockAround(src, 1), null);
});

test("blank lines inside brackets don't end the block", () => {
  const src = lines("instr a(freq: hz) {\n  x = 1\n\n  out = x\n}\n\nhush");
  assert.deepStrictEqual(blockAround(src, 1), { start: 0, end: 4 });
  assert.deepStrictEqual(blockAround(src, 3), { start: 0, end: 4 });
});

test("brackets in comments are ignored", () => {
  const src = lines("play a = [c4] // [\n\nhush");
  assert.deepStrictEqual(blockAround(src, 0), { start: 0, end: 0 });
});
