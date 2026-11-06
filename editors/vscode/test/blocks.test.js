const test = require("node:test");
const assert = require("node:assert");
const { blockAround, nextStatement, paragraphAround } = require("../blocks");

const lines = (text) => text.split("\n");

test("neighbouring lines are separate statements", () => {
  const src = lines("tempo 100bpm\n\nmelody.start\nbass.start\n\nhush");
  assert.deepStrictEqual(blockAround(src, 2), { start: 2, end: 2 });
  assert.deepStrictEqual(blockAround(src, 3), { start: 3, end: 3 });
  assert.deepStrictEqual(blockAround(src, 0), { start: 0, end: 0 });
  assert.strictEqual(blockAround(src, 1), null);
});

test(".method and indented lines continue a statement", () => {
  const src = lines("a.start\nb\n  .reverse()\n  .fast(2)\nc.start");
  assert.deepStrictEqual(blockAround(src, 1), { start: 1, end: 3 });
  assert.deepStrictEqual(blockAround(src, 3), { start: 1, end: 3 });
  assert.deepStrictEqual(blockAround(src, 4), { start: 4, end: 4 });
});

test("any line inside an instrument selects all of it, blank lines or not", () => {
  const src = lines("instr a(freq: hz) {\n  x = 1\n\n  g = grid {\n    1: x...\n  }.over(1bar)\n\n  out = x\n}\n\nhush");
  for (const line of [0, 1, 2, 4, 5, 6, 7, 8]) assert.deepStrictEqual(blockAround(src, line), { start: 0, end: 8 });
  assert.strictEqual(blockAround(src, 9), null);
  assert.deepStrictEqual(blockAround(src, 10), { start: 10, end: 10 });
});

test("brackets in comments and strings are ignored", () => {
  assert.deepStrictEqual(blockAround(lines("a.start // [\nb.start\n"), 1), { start: 1, end: 1 });
  assert.deepStrictEqual(blockAround(lines('a.load("x(.wav")\nb.start'), 1), { start: 1, end: 1 });
});

test("a paragraph is the lines around the cursor up to blank lines", () => {
  const src = lines("tempo 100bpm\n\nmelody.start\nbass.start\n\nhush");
  assert.deepStrictEqual(paragraphAround(src, 3), { start: 2, end: 3 });
  assert.strictEqual(paragraphAround(src, 1), null);
});

test("a paragraph is not cut by blank lines inside brackets", () => {
  const src = lines("instr a(freq: hz) {\n  x = 1\n\n  out = x\n}\n\nhush");
  assert.deepStrictEqual(paragraphAround(src, 3), { start: 0, end: 4 });
});

test("the next statement starts after the current one", () => {
  const src = lines("a.start\n\n// note\nb\n  .fast(2)\nc.start");
  assert.strictEqual(nextStatement(src, 0), 3);
  assert.strictEqual(nextStatement(src, 3), 5);
  assert.strictEqual(nextStatement(src, 4), 5);
  assert.strictEqual(nextStatement(src, 5), null);
});
