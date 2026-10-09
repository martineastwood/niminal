const vscode = require("vscode");
const { Client } = require("./client");
const { blockAround } = require("./blocks");
const { trackDescription, commands: panelCommands } = require("./panel");

let client = null;
let status;
let diagnostics;
let channel;
let transport = null;
let performance = { scenes: [], tracks: [] };
const scenesChanged = new vscode.EventEmitter();
const tracksChanged = new vscode.EventEmitter();
let refreshTimer;

const flash = vscode.window.createTextEditorDecorationType({
  backgroundColor: new vscode.ThemeColor("editor.findMatchHighlightBackground"),
});

function settings() {
  const c = vscode.workspace.getConfiguration("niminal");
  return { port: c.get("port"), token: c.get("token"), quantize: c.get("quantize"), path: c.get("path") };
}

function showStatus() {
  if (!client?.connected) {
    status.text = "$(debug-disconnect) niminal";
    status.tooltip = "Not connected. Click to connect to the daemon.";
    status.command = "niminal.connect";
    return;
  }
  status.command = "niminal.hush";
  status.tooltip = "Connected. Click to hush.";
  if (!transport) {
    status.text = "$(unmute) niminal";
    return;
  }
  const pending = transport.pending ? ` · ${transport.pending} pending` : "";
  status.text = `$(unmute) ${transport.bar}.${Math.floor(transport.beat)} · ${Math.round(transport.bpm)} bpm · ${transport.voices} voices${pending}`;
}

async function connect() {
  if (client?.connected) return client;
  const { port, token } = settings();
  client = new Client({
    port,
    token,
    onNotification(method, params) {
      if (method === "transport") {
        transport = params;
        showStatus();
      } else if (method === "landed") {
        refreshPerformance();
        channel.appendLine(`landed at bar ${params.position.bar} beat ${params.position.beat.toFixed(2)}`);
      } else if (method === "notice") {
        channel.appendLine(`notice: ${params.message ?? JSON.stringify(params)}`);
      }
    },
    onClose() {
      transport = null;
      performance = { scenes: [], tracks: [] };
      scenesChanged.fire();
      tracksChanged.fire();
      showStatus();
    },
  });
  await client.connect();
  await client.call("subscribe", { topics: ["transport", "landed", "notices"] });
  showStatus();
  refreshPerformance();
  return client;
}

async function refreshPerformance() {
  if (!client?.connected) return;
  try {
    const status = await client.call("status");
    performance = { scenes: status.scenes ?? [], tracks: status.tracks ?? [] };
    scenesChanged.fire();
    tracksChanged.fire();
  } catch {
    // the connection went away; onClose clears the panel
  }
}

/** Send a line of code from a panel button or key, as if it had been evaluated in an editor. */
async function sendCode(code) {
  const c = await ensureConnected();
  if (!c) return;
  const params = { source: code };
  const { quantize } = settings();
  if (quantize) params.quantize = quantize;
  try {
    const result = await c.call("eval", params);
    const where = result.id === null ? "nothing to do" : result.in_seconds === 0 ? "applied" : `lands at bar ${result.position.bar} beat ${result.position.beat.toFixed(2)}`;
    vscode.window.setStatusBarMessage(`niminal: ${code} — ${where}`, 2500);
    refreshPerformance();
  } catch (e) {
    vscode.window.showErrorMessage(`niminal: ${e.message}`);
  }
}

async function ensureConnected() {
  try {
    return await connect();
  } catch (e) {
    const choice = await vscode.window.showErrorMessage(`niminal: ${e.message}`, "Start Daemon");
    if (choice === "Start Daemon") startDaemon();
    return null;
  }
}

/** Show the daemon's complaints about code that began at `firstLine` of `document`. */
function reportProblems(document, firstLine, problems, range) {
  const kept = (diagnostics.get(document.uri) ?? []).filter((d) => !range.intersection(d.range));
  const fresh = problems.map((p) => {
    const line = firstLine + (p.line ?? 1) - 1;
    const column = (p.column ?? 1) - 1;
    const length = p.span ? Math.max(1, p.span.end - p.span.start) : 1;
    const at = new vscode.Range(line, column, line, column + length);
    const message = p.help ? `${p.message}\n${p.help}` : p.message;
    return new vscode.Diagnostic(at, message, vscode.DiagnosticSeverity.Error);
  });
  diagnostics.set(document.uri, [...kept, ...fresh]);
}

async function evaluate(editor, range) {
  const c = await ensureConnected();
  if (!c) return;
  const { document } = editor;
  const params = { source: document.getText(range) };
  if (document.uri.scheme === "file") params.dir = require("path").dirname(document.uri.fsPath);
  const { quantize } = settings();
  if (quantize) params.quantize = quantize;
  try {
    const result = await c.call("eval", params);
    reportProblems(document, range.start.line, [], range);
    editor.setDecorations(flash, [range]);
    setTimeout(() => editor.setDecorations(flash, []), 250);
    const where = result.id === null ? "nothing to do" : result.in_seconds === 0 ? "applied" : `lands at bar ${result.position.bar} beat ${result.position.beat.toFixed(2)}`;
    vscode.window.setStatusBarMessage(`niminal: ${where}`, 2500);
  } catch (e) {
    if (e.problems) {
      reportProblems(document, range.start.line, e.problems, range);
      channel.appendLine(e.problems.map((p) => `${p.line}:${p.column} ${p.message}`).join("\n"));
    } else {
      vscode.window.showErrorMessage(`niminal: ${e.message}`);
    }
  }
}

function evalBlock() {
  const editor = vscode.window.activeTextEditor;
  if (!editor) return;
  let range = editor.selection;
  if (range.isEmpty) {
    const lines = editor.document.getText().split("\n");
    const block = blockAround(lines, range.active.line);
    if (!block) return;
    range = new vscode.Range(block.start, 0, block.end, lines[block.end].length);
  } else {
    // Whole lines, so reported positions line up.
    range = new vscode.Range(range.start.line, 0, range.end.line, editor.document.lineAt(range.end.line).text.length);
  }
  return evaluate(editor, range);
}

function evalFile() {
  const editor = vscode.window.activeTextEditor;
  if (!editor) return;
  const last = editor.document.lineCount - 1;
  return evaluate(editor, new vscode.Range(0, 0, last, editor.document.lineAt(last).text.length));
}

async function simple(method) {
  const c = await ensureConnected();
  if (c) await c.call(method).catch((e) => vscode.window.showErrorMessage(`niminal: ${e.message}`));
}

function startDaemon() {
  const { port, token, path } = settings();
  const terminal = vscode.window.createTerminal("niminal daemon");
  terminal.sendText(`${path} daemon --port ${port}${token ? ` --token ${token}` : ""}`);
  terminal.show(true);
}

function activate(context) {
  status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  diagnostics = vscode.languages.createDiagnosticCollection("niminal");
  channel = vscode.window.createOutputChannel("niminal");
  showStatus();
  status.show();
  const command = (name, run) => vscode.commands.registerCommand(name, run);
  context.subscriptions.push(
    status,
    diagnostics,
    channel,
    flash,
    command("niminal.evalBlock", evalBlock),
    command("niminal.evalFile", evalFile),
    command("niminal.hush", () => simple("hush")),
    command("niminal.panic", () => simple("panic")),
    command("niminal.connect", ensureConnected),
    command("niminal.startDaemon", startDaemon),
    vscode.window.registerTreeDataProvider("niminal.scenes", {
      onDidChangeTreeData: scenesChanged.event,
      getChildren: () => performance.scenes.map((name, i) => ({ name, number: i + 1 })),
      getTreeItem: ({ name, number }) => {
        const item = new vscode.TreeItem(name);
        item.description = number <= 9 ? `${process.platform === "darwin" ? "⌘" : "Ctrl+"}${number}` : "";
        item.command = { command: "niminal.launchScene", title: "Launch", arguments: [{ name }] };
        item.iconPath = new vscode.ThemeIcon("play");
        return item;
      },
    }),
    vscode.window.registerTreeDataProvider("niminal.tracks", {
      onDidChangeTreeData: tracksChanged.event,
      getChildren: () => performance.tracks,
      getTreeItem: (track) => {
        const item = new vscode.TreeItem(track.name);
        item.description = trackDescription(track);
        item.iconPath = new vscode.ThemeIcon(track.muted ? "mute" : track.clip ? "play" : "circle-outline");
        return item;
      },
    }),
    command("niminal.launchScene", (scene) => scene && sendCode(panelCommands.launch(scene.name))),
    command("niminal.launchNth", (n) => {
      const name = performance.scenes[n - 1];
      if (name) return sendCode(panelCommands.launch(name));
      vscode.window.setStatusBarMessage(`niminal: no scene number ${n}`, 2000);
    }),
    command("niminal.toggleMute", (track) => track && sendCode(panelCommands.mute(track))),
    command("niminal.toggleSolo", (track) => track && sendCode(panelCommands.solo(track))),
    command("niminal.stopTrack", (track) => track && sendCode(panelCommands.stop(track))),
    { dispose: () => clearInterval(refreshTimer) },
  );
  // Tracks change without any evaluation when a timeline command comes due.
  refreshTimer = setInterval(refreshPerformance, 2000);
}

function deactivate() {
  client?.close();
}

module.exports = { activate, deactivate };
