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

fs.rmSync(target, { recursive: true, force: true });
fs.mkdirSync(target, { recursive: true });
for (const entry of ["package.json", "package-lock.json", "extension.js", "client.js", "blocks.js", "panel.js",
  "language-configuration.json", "syntaxes", "media", "node_modules"]) {
  fs.cpSync(path.join(__dirname, entry), path.join(target, entry), { recursive: true });
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
