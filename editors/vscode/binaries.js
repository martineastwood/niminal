const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

/** Explicit settings win; the macOS download also works when VS Code has no shell PATH. */
function findBinary(program, configured, { platform = process.platform, home = os.homedir(), envPath = process.env.PATH || "", exists = executable } = {}) {
  if (configured && configured !== program) return configured;
  const folders = platform === "darwin" ? ["/Applications/Niminal/bin"] : [];
  folders.push(...envPath.split(path.delimiter).filter(Boolean), path.join(home, ".cargo", "bin"));
  for (const folder of folders) {
    const candidate = path.join(folder, platform === "win32" ? `${program}.exe` : program);
    if (exists(candidate)) return candidate;
  }
  return configured || program;
}

function executable(file) {
  try { fs.accessSync(file, fs.constants.X_OK); return fs.statSync(file).isFile(); }
  catch { return false; }
}

/** The command that opens the Stage app by name: `open -a` on macOS, nothing known elsewhere yet. */
function openStageCommand(app, platform = process.platform) {
  if (!app) return null;
  return platform === "darwin" ? ["open", "-a", app] : null;
}

module.exports = { findBinary, openStageCommand };
