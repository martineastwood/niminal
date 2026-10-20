const vscode = require("vscode");
const net = require("node:net");
const { LanguageClient } = require("vscode-languageclient/node");
const { LiveClient: Client } = require("niminal-client");
const { startLive } = require("./live");
const { blockAround } = require("./blocks");
const { trackDescription } = require("./panel");
const { rewriteLearned } = require("./learn");

let client = null;
let live = null;
let connecting = null;
let languageServer = null;
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
  return { port: c.get("port"), token: c.get("token"), quantize: c.get("quantize"), path: c.get("path"), livePath: c.get("livePath"), midi: c.get("midi"), oscPort: c.get("oscPort") };
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

/** The daemon heard a control change for `midi.learn`: write it into the code that asked. */
async function applyLearned(control, source) {
  channel.appendLine(`learned: ctl ${control} = ${source}`);
  for (const document of vscode.workspace.textDocuments) {
    if (document.languageId !== "niminal") continue;
    const text = document.getText();
    const next = rewriteLearned(text, control, source);
    if (next === null) continue;
    const edit = new vscode.WorkspaceEdit();
    edit.replace(document.uri, new vscode.Range(document.positionAt(0), document.positionAt(text.length)), next);
    await vscode.workspace.applyEdit(edit);
  }
}

async function connect() {
  if (client?.connected) return client;
  if (connecting) return connecting;
  connecting = connectLive();
  try {
    return await connecting;
  } catch (error) {
    client?.close();
    live?.dispose();
    live = null;
    throw error;
  } finally {
    connecting = null;
  }
}

async function connectLive() {
  const config = settings();
  const { token } = config;
  live?.dispose();
  live = startLive({ ...config, path: config.livePath }, (text) => channel.appendLine(`live: ${text}`));
  const port = await live.ready;
  client = new Client({
    port,
    token,
    client: "niminal-vscode",
    onNotification(method, params) {
      if (method === "transport") {
        transport = params;
        showStatus();
      } else if (method === "landed") {
        refreshPerformance();
        channel.appendLine(`landed at bar ${params.position.bar} beat ${params.position.beat.toFixed(2)}`);
      } else if (method === "learned") {
        applyLearned(params.control, params.source);
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
  await client.subscribe(["transport", "landed", "notices", "learned"]);
  showStatus();
  refreshPerformance();
  return client;
}

async function refreshPerformance() {
  if (!client?.connected) return;
  try {
    const status = await client.status();
    performance = { scenes: status.scenes ?? [], tracks: status.tracks ?? [] };
    scenesChanged.fire();
    tracksChanged.fire();
  } catch {
    // the connection went away; onClose clears the panel
  }
}

/** Typed performance call (launch, mute, …) with status feedback. */
async function sendApi(label, run) {
  const c = await ensureConnected();
  if (!c) return;
  const { quantize } = settings();
  try {
    const result = await run(c, quantize);
    const where = result.id === null ? "nothing to do" : result.in_seconds === 0 ? "applied" : `lands at bar ${result.position.bar} beat ${result.position.beat.toFixed(2)}`;
    vscode.window.setStatusBarMessage(`niminal: ${label} — ${where}`, 2500);
    refreshPerformance();
  } catch (e) {
    vscode.window.showErrorMessage(`niminal: ${e.message}`);
  }
}

async function ensureConnected() {
  try {
    return await connect();
  } catch (e) {
    // A daemon that is listening but won't talk is not helped by starting another one:
    // say what it said instead of asking for a daemon that is already there.
    if (await listening(settings().port)) return reportConnectError(e);
    const choice = await vscode.window.showErrorMessage(`niminal: ${e.message}`, "Start Daemon");
    if (choice === "Start Daemon") return startDaemon();
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
  const { quantize } = settings();
  const opts = { quantize };
  if (document.uri.scheme === "file") opts.dir = require("path").dirname(document.uri.fsPath);
  try {
    const result = await c.eval(document.getText(range), opts);
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

/** Whether something is listening on `port` of this machine. */
function listening(port, timeout = 500) {
  return new Promise((resolve) => {
    const socket = net.connect({ port, host: "127.0.0.1" });
    const answer = (yes) => {
      socket.destroy();
      resolve(yes);
    };
    socket.setTimeout(timeout, () => answer(false));
    socket.once("connect", () => answer(true));
    socket.once("error", () => answer(false));
  });
}

/** Wait for a daemon to answer on `port`, turning a timeout into something readable. */
async function waitForDaemon(port, timeout = 20000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await listening(port)) return;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`nothing answered on port ${port}; the daemon terminal says why`);
}

function reportConnectError(e) {
  vscode.window.showErrorMessage(`niminal: ${e.message}`);
  return null;
}

/** Start a daemon in a terminal, then connect to it, so the editor uses what it started. */
async function startDaemon() {
  const { port, token, path } = settings();
  if (await listening(port)) {
    vscode.window.showInformationMessage(`niminal: a daemon is already listening on port ${port}`);
  } else {
    const terminal = vscode.window.createTerminal("niminal daemon");
    terminal.sendText(`${path} daemon --port ${port}${token ? ` --token ${token}` : ""}`);
    terminal.show(true);
    channel.appendLine(`starting the daemon on port ${port}`);
    try {
      await waitForDaemon(port);
    } catch (e) {
      return reportConnectError(e);
    }
  }
  try {
    return await connect();
  } catch (e) {
    return reportConnectError(e);
  }
}

// Errors, completion, hover and go-to-definition come from `nimlive lsp`, the same
// binary as the live sequencer, so they always agree with it about the language.
function startLanguageServer() {
  const server = new LanguageClient(
    "niminal",
    "niminal language server",
    { command: settings().livePath, args: ["lsp"] },
    { documentSelector: [{ language: "niminal" }] },
  );
  languageServer = server;
  server.start().catch((e) => {
    channel.appendLine(`language server: ${e.message}`);
    vscode.window.showWarningMessage(`niminal: can't start \`${settings().livePath} lsp\`. Set niminal.livePath to the nimlive executable.`);
  });
}

async function restartLanguageServer() {
  const old = languageServer;
  languageServer = null;
  await old?.stop().catch(() => {});
  startLanguageServer();
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
    command("niminal.restartLanguageServer", restartLanguageServer),
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
    command("niminal.launchScene", (scene) => scene && sendApi(`${scene.name}.start`, (c, q) => c.launch(scene.name, { quantize: q }))),
    command("niminal.launchNth", (n) => {
      const name = performance.scenes[n - 1];
      if (name) return sendApi(`${name}.start`, (c, q) => c.launch(name, { quantize: q }));
      vscode.window.setStatusBarMessage(`niminal: no scene number ${n}`, 2000);
    }),
    command("niminal.toggleMute", (track) => {
      if (!track) return;
      return track.muted
        ? sendApi(`${track.name}.unmute`, (c, q) => c.unmute(track.name, { quantize: q }))
        : sendApi(`${track.name}.mute`, (c, q) => c.mute(track.name, { quantize: q }));
    }),
    command("niminal.toggleSolo", (track) => {
      if (!track) return;
      return track.soloed
        ? sendApi(`${track.name}.unsolo`, (c, q) => c.unsolo(track.name, { quantize: q }))
        : sendApi(`${track.name}.solo`, (c, q) => c.solo(track.name, { quantize: q }));
    }),
    command("niminal.stopTrack", (track) => track && sendApi(`${track.name}.stop`, (c, q) => c.stop(track.name, { quantize: q }))),
    { dispose: () => clearInterval(refreshTimer) },
  );
  // Tracks change without any evaluation when a timeline command comes due.
  refreshTimer = setInterval(refreshPerformance, 2000);
  startLanguageServer();
}

function deactivate() {
  client?.close();
  live?.dispose();
  return languageServer?.stop();
}

module.exports = { activate, deactivate };
