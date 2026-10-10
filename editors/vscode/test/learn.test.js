const test = require("node:test");
const assert = require("node:assert");
const { rewriteLearned } = require("../learn");

test("midi.learn becomes the control change that was heard", () => {
  const text = "ctl cutoff = midi.learn.range(200hz..4khz)\nctl drive = midi.learn\n";
  assert.strictEqual(
    rewriteLearned(text, "cutoff", "midi.cc(21)"),
    "ctl cutoff = midi.cc(21).range(200hz..4khz)\nctl drive = midi.learn\n",
  );
  assert.strictEqual(rewriteLearned(text, "drive", "midi.cc(7)"), "ctl cutoff = midi.learn.range(200hz..4khz)\nctl drive = midi.cc(7)\n");
});

test("only the named control is rewritten, and a file without it is left alone", () => {
  assert.strictEqual(rewriteLearned("ctl cutoff2 = midi.learn", "cutoff", "midi.cc(1)"), null);
  assert.strictEqual(rewriteLearned("ctl cutoff = midi.cc(5)", "cutoff", "midi.cc(1)"), null);
});
