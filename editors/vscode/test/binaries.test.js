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

test("Stage picks the installed prod bundle on macOS, avoiding stale LaunchServices copies", () => {
  assert.deepEqual(
    openStageCommand("Niminal Stage", {
      platform: "darwin",
      mtime: (p) => (p === "/Applications/Niminal/Niminal Stage.app" ? 100 : null),
    }),
    ["open", "-a", "/Applications/Niminal/Niminal Stage.app"],
  );
  assert.deepEqual(
    openStageCommand("Niminal Stage", { platform: "darwin", mtime: () => null }),
    ["open", "-a", "Niminal Stage"],
  );
  assert.equal(openStageCommand("Niminal Stage", { platform: "linux" }), null);
  assert.equal(openStageCommand("", { platform: "darwin" }), null);
});

test("Stage picks a newer workspace dev bundle over prod, but never an older dev bundle", () => {
  const dev = "/repo/stage/src-tauri/target/release/bundle/macos/niminal stage.app";
  const prod = "/Applications/Niminal/Niminal Stage.app";
  assert.deepEqual(
    openStageCommand("Niminal Stage", {
      platform: "darwin",
      workspaces: ["/repo"],
      mtime: (p) => (p === dev ? 200 : p === prod ? 100 : null),
    }),
    ["open", "-a", dev],
  );
  assert.deepEqual(
    openStageCommand("Niminal Stage", {
      platform: "darwin",
      workspaces: ["/repo"],
      mtime: (p) => (p === dev ? 50 : p === prod ? 100 : null),
    }),
    ["open", "-a", prod],
  );
});

test("explicit stageApp setting is used verbatim", () => {
  assert.deepEqual(
    openStageCommand("/custom/Stage.app", { platform: "darwin", workspaces: ["/repo"], mtime: () => 100 }),
    ["open", "-a", "/custom/Stage.app"],
  );
});
