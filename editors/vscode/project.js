const fs = require("node:fs");
const path = require("node:path");

/** The module names a source file `use`s, ignoring aliases and comments. */
function usedModules(text) {
  const names = [];
  for (const line of text.split("\n")) {
    const m = /^\s*use\s+([\w/]+)/.exec(line);
    if (m) names.push(m[1]);
  }
  return names;
}

/**
 * The files of the project in `dir`. With a `main.nml` that is the file and every module it reaches
 * through `use` (found next to the file using it, or in `lib`); without one, every `.nml` in the folder.
 * `entry` is the file to evaluate to play the project, or null when each file stands alone.
 */
function projectFiles(dir) {
  const main = path.join(dir, "main.nml");
  if (!fs.existsSync(main)) {
    const files = fs.readdirSync(dir).filter((f) => f.endsWith(".nml")).sort().map((f) => path.join(dir, f));
    return { entry: null, files };
  }
  const files = [];
  const visit = (file) => {
    if (files.includes(file)) return;
    files.push(file);
    for (const name of usedModules(fs.readFileSync(file, "utf8"))) {
      const found = [path.join(path.dirname(file), `${name}.nml`), path.join(dir, "lib", `${name}.nml`)].find((p) => fs.existsSync(p));
      if (found) visit(found);
    }
  };
  visit(main);
  return { entry: main, files };
}

module.exports = { projectFiles, usedModules };
