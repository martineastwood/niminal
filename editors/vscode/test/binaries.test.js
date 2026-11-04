const { test } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { findBinary, serverCommand, backgroundServerArgs } = require("../binaries");

test("Finder-launched VS Code discovers the macOS tools without PATH", () => {
  assert.equal(findBinary("niminal-live", "niminal-live", {
    platform: "darwin", home: "/empty", exists: p => p === "/Applications/Niminal/bin/niminal-live",
  }), "/Applications/Niminal/bin/niminal-live");
});

test("explicit executable settings take precedence over the download", () => {
  assert.equal(findBinary("niminal", "/custom tools/niminal", { exists: () => true }), "/custom tools/niminal");
});

test("development installations and bare command names still work", () => {
  assert.equal(findBinary("niminal", "niminal", {
    platform: "linux", home: "/developer", exists: p => p === "/developer/.cargo/bin/niminal",
  }), "/developer/.cargo/bin/niminal");
  assert.equal(findBinary("niminal-live", "niminal-live", { exists: () => false }), "niminal-live");
});

test("terminal command preserves paths and tokens as literal shell arguments", () => {
  const value = "a path/with 'quotes' and $(echo substituted)";
  const command = serverCommand("printf", 7401, value, "darwin");
  // Replace printf's format with a fixture that prints the argument array as JSON.
  const text = command.replace("'printf'", "node -e 'process.stdout.write(JSON.stringify(process.argv.slice(1)))' --");
  const args = JSON.parse(execFileSync("/bin/sh", ["-c", text], { encoding: "utf8" }));
  assert.deepEqual(args, ["server", "--port", "7401", "--token", value]);
});

test("a background server stops itself when unused, and takes the port and token", () => {
  assert.deepEqual(backgroundServerArgs(7402, null), ["server", "--port", "7402", "--exit-when-unused"]);
  assert.deepEqual(backgroundServerArgs("7402", "a b"), ["server", "--port", "7402", "--exit-when-unused", "--token", "a b"]);
});
