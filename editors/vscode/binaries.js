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

const STAGE_APP = "Niminal Stage";
const PROD_BUNDLES = [
  "/Applications/Niminal/Niminal Stage.app",
  "/Applications/Niminal Stage.app",
];

function appModifiedMs(bundle) {
  try {
    const bin = path.join(bundle, "Contents", "MacOS", "niminal-stage");
    return fs.statSync(fs.existsSync(bin) ? bin : bundle).mtimeMs;
  } catch {
    return null;
  }
}

/**
 * The command that opens Stage on macOS (`null` elsewhere). When left at the default name,
 * opens the newest bundle among any dev build in the open workspace and the installed prod app,
 * avoiding LaunchServices picking stale copies from elsewhere on disk.
 */
function openStageCommand(app, { platform = process.platform, workspaces = [], mtime = appModifiedMs } = {}) {
  if (!app || platform !== "darwin") return null;
  if (app !== STAGE_APP) return ["open", "-a", app];

  const candidates = [];
  for (const ws of workspaces) {
    for (const profile of ["release", "debug"]) {
      candidates.push(path.join(ws, "stage", "src-tauri", "target", profile, "bundle", "macos", "niminal stage.app"));
    }
  }
  candidates.push(...PROD_BUNDLES);

  let best = null;
  let bestTime = -Infinity;
  for (const bundle of candidates) {
    const t = mtime(bundle);
    if (t !== null && t > bestTime) {
      best = bundle;
      bestTime = t;
    }
  }
  return ["open", "-a", best ?? app];
}

module.exports = { findBinary, openStageCommand };
