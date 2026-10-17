// Installs this folder as a VS Code extension by copying it into the user's
// extensions folder. Reload VS Code afterwards. Run again to update.
//   node install.js [--cursor]

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const pkg = require("./package.json");
const editor = process.argv.includes("--cursor") ? ".cursor" : ".vscode";
const root = path.join(os.homedir(), editor, "extensions");
const id = `${pkg.publisher}.${pkg.name}`;
const folder = `${id}-${pkg.version}`;
const target = path.join(root, folder);

// Drop any older versions of this extension so only the fresh copy is loaded.
for (const name of fs.existsSync(root) ? fs.readdirSync(root) : []) {
  if (name.startsWith(`${id}-`) && name !== folder) {
    fs.rmSync(path.join(root, name), { recursive: true, force: true });
  }
}

fs.rmSync(target, { recursive: true, force: true });
fs.mkdirSync(target, { recursive: true });
for (const entry of [
  "package.json",
  "package-lock.json",
  "extension.js",
  "blocks.js",
  "panel.js",
  "learn.js",
  "language-configuration.json",
  "syntaxes",
  "media",
  "node_modules",
]) {
  const src = path.join(__dirname, entry);
  if (!fs.existsSync(src)) continue;
  // dereference so file: deps (niminal-client) are real copies, not broken symlinks
  fs.cpSync(src, path.join(target, entry), { recursive: true, dereference: true });
}

const registry = path.join(root, "extensions.json");
const installed = fs.existsSync(registry) ? JSON.parse(fs.readFileSync(registry, "utf8")) : [];
const others = installed.filter((e) => e.identifier.id !== id);
others.push({
  identifier: { id },
  version: pkg.version,
  location: { $mid: 1, path: target, scheme: "file" },
  relativeLocation: folder,
  metadata: { installedTimestamp: Date.now(), source: "vsix" },
});
fs.writeFileSync(registry, JSON.stringify(others));
console.log(`installed ${id} ${pkg.version} into ${target}; reload the editor`);
