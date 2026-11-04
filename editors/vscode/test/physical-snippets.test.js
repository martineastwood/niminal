const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const root = path.join(__dirname, "..");
const niminal = process.env.NIMINAL_BIN ?? path.join(root, "../../target/debug/niminal");

test("the contributed physical-model starters compile with the language", () => {
  const pkg = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
  const contribution = pkg.contributes.snippets.find(s => s.language === "niminal");
  assert.ok(contribution, "Niminal snippets must be contributed to the editor");
  const snippets = JSON.parse(fs.readFileSync(path.join(root, contribution.path), "utf8"));
  const prefixes = Object.values(snippets).map(s => s.prefix);
  assert.ok(prefixes.includes("instr-bowed") && prefixes.includes("instr-reed"));
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "niminal-physical-snippets-"));
  try {
    for (const snippet of Object.values(snippets)) {
      const body = snippet.body.join("\n").replace(/\$\{\d+:([^}]+)\}/g, "$1").replace(/\$\d+/g, "");
      const file = path.join(dir, `${snippet.prefix}.nml`);
      fs.writeFileSync(file, body);
      const result = spawnSync(niminal, ["explain", file], { encoding: "utf8", timeout: 30000 });
      assert.equal(result.status, 0, `${snippet.prefix}: ${result.error ?? result.stderr}`);
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
