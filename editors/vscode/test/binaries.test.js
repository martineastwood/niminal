const { test } = require("node:test");
const assert = require("node:assert/strict");
const { findBinary, openStageCommand } = require("../binaries");

test("Finder-launched VS Code discovers the macOS tools without PATH", () => {
  assert.equal(findBinary("niminal", "niminal", {
    platform: "darwin", home: "/empty", exists: p => p === "/Applications/Niminal/bin/niminal",
  }), "/Applications/Niminal/bin/niminal");
});

test("explicit executable settings take precedence over the download", () => {
  assert.equal(findBinary("niminal", "/custom tools/niminal", { exists: () => true }), "/custom tools/niminal");
});

test("development installations and bare command names still work", () => {
  assert.equal(findBinary("niminal", "niminal", {
    platform: "linux", home: "/developer", exists: p => p === "/developer/.cargo/bin/niminal",
  }), "/developer/.cargo/bin/niminal");
  assert.equal(findBinary("niminal", "niminal", { exists: () => false }), "niminal");
});

test("Stage is opened by name on macOS, and not guessed at elsewhere", () => {
  assert.deepEqual(openStageCommand("Niminal Stage", "darwin"), ["open", "-a", "Niminal Stage"]);
  assert.equal(openStageCommand("Niminal Stage", "linux"), null);
  assert.equal(openStageCommand("", "darwin"), null);
});
