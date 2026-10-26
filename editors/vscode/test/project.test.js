const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { projectFiles, usedModules } = require("../project");

function folder(files) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "niminal-project-"));
  for (const [name, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(dir, name)), { recursive: true });
    fs.writeFileSync(path.join(dir, name), text);
  }
  return dir;
}

test("usedModules reads names and ignores aliases", () => {
  assert.deepEqual(usedModules("use a\n  use score/b as x\n// use c\nlet use_d = 1"), ["a", "score/b"]);
});

test("main.nml leads and its modules follow, once each", () => {
  const dir = folder({
    "main.nml": "use drums\nuse std/filters\nuse bass\n",
    "drums.nml": "use bass\n",
    "bass.nml": "",
    "lib/pads.nml": "",
    "scratch.nml": "",
  });
  const { entry, files } = projectFiles(dir);
  assert.equal(entry, path.join(dir, "main.nml"));
  assert.deepEqual(files.map((f) => path.basename(f)), ["main.nml", "drums.nml", "bass.nml"]);
});

test("a module in lib is found", () => {
  const dir = folder({ "main.nml": "use pads\n", "lib/pads.nml": "" });
  assert.deepEqual(projectFiles(dir).files.map((f) => path.relative(dir, f)), ["main.nml", path.join("lib", "pads.nml")]);
});

test("without main.nml every file is a tab", () => {
  const dir = folder({ "b.nml": "", "a.nml": "", "notes.txt": "" });
  const { entry, files } = projectFiles(dir);
  assert.equal(entry, null);
  assert.deepEqual(files.map((f) => path.basename(f)), ["a.nml", "b.nml"]);
});
