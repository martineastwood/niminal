const { spawn } = require("node:child_process");

/** Each editor owns a live sequencer; only generic events cross into the daemon. */
function startLive({ path, port, token, midi, oscPort }, log, spawnProcess = spawn) {
  const args = ["live", "--engine-port", String(port), "--port", "0", "--parent-stdio"];
  if (token) args.push("--token", token);
  if (midi !== null && midi !== undefined) args.push("--midi", midi);
  if (oscPort !== null && oscPort !== undefined) args.push("--osc-port", String(oscPort));
  const child = spawnProcess(path, args, { stdio: ["pipe", "pipe", "pipe"] });
  const ready = new Promise((resolve, reject) => {
    let output = "";
    const timer = setTimeout(() => fail(new Error("live sequencer startup timed out")), 15000);
    function fail(error) {
      clearTimeout(timer);
      child.kill();
      reject(error);
    }
    child.once("error", fail);
    child.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`live sequencer exited (${code})`));
    });
    child.stderr.on("data", (data) => log(data.toString().trimEnd()));
    child.stdout.on("data", (data) => {
      output += data.toString();
      const match = output.match(/niminal live listening on ws:\/\/127\.0\.0\.1:(\d+)\r?\n/);
      if (match) {
        clearTimeout(timer);
        resolve(Number(match[1]));
      }
    });
  });
  return { ready, dispose: () => child.kill() };
}

module.exports = { startLive };
