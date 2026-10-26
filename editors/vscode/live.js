const { spawn } = require("node:child_process");

/** Each editor owns a live sequencer; only generic events cross into the server. */
function startLive({ path, port, token, midi, oscPort }, log, spawnProcess = spawn) {
  const args = ["live", "--engine-port", String(port), "--port", "0", "--parent-stdio"];
  if (token) args.push("--token", token);
  if (midi !== null && midi !== undefined) args.push("--midi", midi);
  if (oscPort !== null && oscPort !== undefined) args.push("--osc-port", String(oscPort));
  // An editor's sequencer is not the server, so it says so in the process list.
  const child = spawnProcess(path, args, { stdio: ["pipe", "pipe", "pipe"], argv0: "niminal-live" });
  const ready = new Promise((resolve, reject) => {
    let output = "";
    let reason = "";
    const timer = setTimeout(() => fail(new Error("live sequencer startup timed out")), 15000);
    function fail(error) {
      clearTimeout(timer);
      child.kill();
      reject(error);
    }
    child.once("error", fail);
    child.once("exit", (code) => {
      clearTimeout(timer);
      // The last thing it said on stderr is why it stopped.
      reject(new Error(`live sequencer exited (${code})${reason ? `: ${reason}` : ""}`));
    });
    child.stderr.on("data", (data) => {
      const text = data.toString().trimEnd();
      if (text) reason = text.split("\n").pop();
      log(text);
    });
    child.stdout.on("data", (data) => {
      output += data.toString();
      const match = output.match(/niminal-live listening on ws:\/\/127\.0\.0\.1:(\d+)\r?\n/);
      if (match) {
        clearTimeout(timer);
        resolve(Number(match[1]));
      }
    });
  });
  return { ready, dispose: () => child.kill() };
}

module.exports = { startLive };
