const vscode = require("vscode");
const net = require("node:net");
const fs = require("node:fs");
const os = require("node:os");
const nodePath = require("node:path");
const { spawn } = require("node:child_process");
const { LanguageClient } = require("vscode-languageclient/node");
const { LiveClient: Client } = require("./live-client");
const { findBinary, openStageCommand } = require("./binaries");
const { releaseMismatch } = require("./compat");
const { blockAround } = require("./blocks");
const { clipDescription, sceneDescription, silencers } = require("./panel");
const { rewriteLearned } = require("./learn");
const { projectFiles } = require("./project");

let client = null;
let connecting = null;
let languageServer = null;
let status;
let diagnostics;
let channel;
let transport = null;
let peaks = null;
let performance = { scenes: [], clips: [] };
let pendingChanges = [];
const scenesChanged = new vscode.EventEmitter();
const clipsChanged = new vscode.EventEmitter();
const pendingChanged = new vscode.EventEmitter();
let refreshTimer;
let intentionalClose = false;
let reconnectTimer = null;
const MAX_RECONNECT_ATTEMPTS = 5;
let reconnectAttempts = 0;

const flash = vscode.window.createTextEditorDecorationType({
  backgroundColor: new vscode.ThemeColor("editor.findMatchHighlightBackground"),
});

function settings() {
  const c = vscode.workspace.getConfiguration("niminal");
  return { port: c.get("port"), token: c.get("token"), quantize: c.get("quantize"), stageApp: c.get("stageApp"), path: findBinary("niminal", c.get("path")) };
}

const METER_BLOCKS = " ▁▂▃▄▅▆▇█";

function peakBar(values) {
  if (!values || values.length === 0) return "";
  return values.map((v) => METER_BLOCKS[Math.min(Math.round(Math.max(0, v) * 8), 8)]).join("");
}

function showStatus() {
  if (!client?.connected) {
    const reconnecting = reconnectTimer !== null;
    status.text = reconnecting ? "$(sync~spin) niminal" : "$(debug-disconnect) niminal";
    status.tooltip = reconnecting ? "Reconnecting…" : "Not connected. Click to connect to Niminal Stage.";
    status.command = reconnecting ? undefined : "niminal.connect";
    return;
  }
  status.command = "niminal.hush";
  status.tooltip = "Connected. Click to hush.";
  if (!transport) {
    status.text = "$(unmute) niminal";
    return;
  }
  const meter = peakBar(peaks);
  const pending = transport.pending ? ` · ${transport.pending} pending` : "";
  const silenced = silencers(performance.clips);
  status.text = `$(unmute) ${transport.bar}.${Math.floor(transport.beat)} · ${Math.round(transport.bpm)} bpm · ${transport.voices} voices${meter ? ` ${meter}` : ""}${pending}${silenced ? ` · ${silenced}` : ""}`;
}

/** Stage heard a control change for `midi.learn`: write it into the code that asked. */
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
    intentionalClose = true;
    client?.close();
    intentionalClose = false;
    throw error;
  } finally {
    connecting = null;
  }
}

async function connectLive() {
  const { port, token } = settings();
  intentionalClose = false;
  client = new Client({
    port,
    token,
    client: "niminal-vscode",
    onNotification(method, params) {
      if (method === "transport") {
        transport = params;
        showStatus();
      } else if (method === "meters") {
        peaks = params.peaks;
      } else if (method === "pending") {
        pendingChanges = params.changes ?? [];
        pendingChanged.fire();
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
      peaks = null;
      performance = { scenes: [], clips: [] };
      pendingChanges = [];
      scenesChanged.fire();
      clipsChanged.fire();
      pendingChanged.fire();
      showStatus();
      if (!intentionalClose) scheduleReconnect();
    },
  });
  const hello = await client.connect();
  const mismatch = releaseMismatch(hello.release, require("./package.json").version);
  if (mismatch) {
    channel.appendLine(mismatch);
    vscode.window.showWarningMessage(`niminal: ${mismatch}`);
  }
  await client.subscribe(["transport", "landed", "notices", "learned", "meters", "pending"]);
  reconnectAttempts = 0;
  clearReconnectTimer();
  showStatus();
  refreshPerformance();
  return client;
}

function clearReconnectTimer() {
  if (reconnectTimer !== null) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
}

function scheduleReconnect() {
  if (reconnectTimer !== null) return;
  if (reconnectAttempts >= MAX_RECONNECT_ATTEMPTS) {
    channel.appendLine("gave up reconnecting after several attempts");
    reconnectAttempts = 0;
    return;
  }
  const delay = Math.min(1000 * 2 ** reconnectAttempts, 8000);
  reconnectAttempts++;
  channel.appendLine(`reconnecting in ${delay}ms (attempt ${reconnectAttempts}/${MAX_RECONNECT_ATTEMPTS})`);
  showStatus();
  reconnectTimer = setTimeout(async () => {
    reconnectTimer = null;
    try {
      await connect();
      vscode.window.setStatusBarMessage("niminal: reconnected", 3000);
    } catch {
      scheduleReconnect();
    }
  }, delay);
}

async function refreshPerformance() {
  if (!client?.connected) return;
  try {
    const status = await client.status();
    performance = { scenes: status.scenes ?? [], clips: status.clips ?? [] };
    scenesChanged.fire();
    clipsChanged.fire();
    showStatus();
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
    // Something that is listening but won't talk is not helped by opening Stage again:
    // say what it said instead.
    if (await listening(settings().port)) return reportConnectError(e);
    return openStage();
  }
}

/** Open Niminal Stage, which has the engine and the sequencer in it, and connect once it is listening. */
async function openStage() {
  const { port, stageApp } = settings();
  const choice = await vscode.window.showInformationMessage(
    `niminal: Niminal Stage is not running. The code you evaluate plays through it.`,
    "Open Stage",
  );
  if (choice !== "Open Stage") return null;
  const command = openStageCommand(stageApp);
  if (!command) {
    vscode.window.showWarningMessage("niminal: open Niminal Stage yourself, then evaluate again.");
    return null;
  }
  channel.appendLine(`opening Niminal Stage: ${command.join(" ")}`);
  const child = spawn(command[0], command.slice(1), { detached: true, stdio: "ignore" });
  const failed = new Promise((_, reject) => {
    child.once("error", (e) => reject(new Error(`could not open ${stageApp}: ${e.message}. Set niminal.stageApp to its name.`)));
    child.once("exit", (code) => code && reject(new Error(`could not open ${stageApp} (exit ${code}). Set niminal.stageApp to its name.`)));
  });
  failed.catch(() => {});
  child.unref();
  try {
    await Promise.race([waitForStage(port), failed]);
    return await connect();
  } catch (e) {
    return reportConnectError(e);
  }
}

/** Show Stage's complaints about code that began at `firstLine` of `document`. */
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

/** Open a folder's files as tabs, then play it: evaluate `main.nml`, or each file in turn without one. */
async function openProject() {
  const [folder] =
    (await vscode.window.showOpenDialog({ canSelectFolders: true, canSelectFiles: false, openLabel: "Open Project" })) ?? [];
  if (!folder) return;
  const { entry, files } = projectFiles(folder.fsPath);
  if (files.length === 0) return vscode.window.showErrorMessage("niminal: no .nml files in that folder");
  const editors = [];
  for (const file of files) {
    const document = await vscode.workspace.openTextDocument(file);
    editors.push(await vscode.window.showTextDocument(document, { preview: false, preserveFocus: true }));
  }
  const first = editors[0];
  await vscode.window.showTextDocument(first.document, { preview: false });
  for (const editor of entry ? [first] : editors) {
    const last = editor.document.lineCount - 1;
    await evaluate(editor, new vscode.Range(0, 0, last, editor.document.lineAt(last).text.length));
  }
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

/** Wait for a server to answer on `port`, turning a timeout into something readable. */
async function waitForStage(port, timeout = 30000, failure = `nothing answered on port ${port}; is Niminal Stage open?`) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await listening(port)) return;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(failure);
}

function reportConnectError(e) {
  vscode.window.showErrorMessage(`niminal: ${e.message}`);
  return null;
}

// Errors, completion, hover and go-to-definition come from `niminal lsp`, which comes in the same download as
// Stage, so they agree about the language. Stage need not be running.
function startLanguageServer() {
  const server = new LanguageClient(
    "niminal",
    "niminal language server",
    { command: settings().path, args: ["lsp"] },
    { documentSelector: [{ language: "niminal" }] },
  );
  languageServer = server;
  server.start().catch((e) => {
    channel.appendLine(`language server: ${e.message}`);
    vscode.window.showWarningMessage(`niminal: can't start \`${settings().path} lsp\`. Install niminal from the Niminal download, or set niminal.path to the niminal program.`);
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
    command("niminal.openProject", openProject),
    command("niminal.hush", () => simple("hush")),
    command("niminal.panic", () => simple("panic")),
    command("niminal.connect", ensureConnected),
    command("niminal.restartLanguageServer", restartLanguageServer),
    vscode.window.registerTreeDataProvider("niminal.scenes", {
      onDidChangeTreeData: scenesChanged.event,
      getChildren: () => performance.scenes.map((scene, i) => ({ ...scene, number: i + 1 })),
      getTreeItem: (scene) => {
        const { name, number } = scene;
        const item = new vscode.TreeItem(name);
        const shortcut = number <= 9 ? `${process.platform === "darwin" ? "⌘" : "Ctrl+"}${number}` : "";
        item.description = sceneDescription(scene, shortcut);
        item.tooltip = `${name}: ${scene.clips.join(", ") || "no clips"}`;
        item.command = { command: "niminal.launchScene", title: "Launch", arguments: [{ name }] };
        item.iconPath = new vscode.ThemeIcon(scene.active ? "debug-start" : "play");
        return item;
      },
    }),
    vscode.window.registerTreeDataProvider("niminal.clips", {
      onDidChangeTreeData: clipsChanged.event,
      getChildren: () => performance.clips,
      getTreeItem: (node) => {
        const item = new vscode.TreeItem(node.name);
        item.description = clipDescription(node);
        item.iconPath = new vscode.ThemeIcon(node.muted ? "mute" : node.running ? "play" : "circle-outline");
        return item;
      },
    }),
    command("niminal.launchScene", (scene) => scene && sendApi(`${scene.name}.start`, (c, q) => c.launch(scene.name, { quantize: q }))),
    command("niminal.launchNth", (n) => {
      const name = performance.scenes[n - 1]?.name;
      if (name) return sendApi(`${name}.start`, (c, q) => c.launch(name, { quantize: q }));
      vscode.window.setStatusBarMessage(`niminal: no scene number ${n}`, 2000);
    }),
    command("niminal.stopScene", (scene) => scene && sendApi(`${scene.name}.stop`, (c, q) => c.stop(scene.name, { quantize: q }))),
    command("niminal.toggleSceneMute", (scene) => {
      if (!scene) return;
      const clips = performance.clips.filter((c) => scene.clips.includes(c.name));
      return clips.length > 0 && clips.every((c) => c.muted)
        ? sendApi(`${scene.name}.unmute`, (c, q) => c.unmute(scene.name, { quantize: q }))
        : sendApi(`${scene.name}.mute`, (c, q) => c.mute(scene.name, { quantize: q }));
    }),
    command("niminal.soloScene", (scene) => scene && sendApi(`${scene.name}.solo`, (c, q) => c.solo(scene.name, { quantize: q }))),
    command("niminal.toggleMute", (node) => {
      if (!node) return;
      return node.muted
        ? sendApi(`${node.name}.unmute`, (c, q) => c.unmute(node.name, { quantize: q }))
        : sendApi(`${node.name}.mute`, (c, q) => c.mute(node.name, { quantize: q }));
    }),
    command("niminal.toggleSolo", (node) => {
      if (!node) return;
      return node.soloed
        ? sendApi(`${node.name}.unsolo`, (c, q) => c.unsolo(node.name, { quantize: q }))
        : sendApi(`${node.name}.solo`, (c, q) => c.solo(node.name, { quantize: q }));
    }),
    command("niminal.stopClip", (node) => node && sendApi(`${node.name}.stop`, (c, q) => c.stop(node.name, { quantize: q }))),
    command("niminal.cancelPending", async (change) => {
      if (!change || !client?.connected) return;
      try {
        await client.cancel(change.id);
        vscode.window.setStatusBarMessage(`niminal: cancelled change ${change.id}`, 2000);
        refreshPerformance();
      } catch (e) {
        vscode.window.showErrorMessage(`niminal: ${e.message}`);
      }
    }),
    vscode.window.registerTreeDataProvider("niminal.pending", {
      onDidChangeTreeData: pendingChanged.event,
      getChildren: () => pendingChanges,
      getTreeItem: (change) => {
        const item = new vscode.TreeItem(`#${change.id}`);
        item.description = `bar ${change.position.bar} beat ${change.position.beat.toFixed(1)} · ${change.changes} change${change.changes === 1 ? "" : "s"}`;
        item.iconPath = new vscode.ThemeIcon("clock");
        return item;
      },
    }),
    { dispose: () => clearInterval(refreshTimer) },
    { dispose: () => clearReconnectTimer() },
  );
  // Parts change without any evaluation when a timeline command comes due.
  refreshTimer = setInterval(refreshPerformance, 2000);
  startLanguageServer();
}

function deactivate() {
  intentionalClose = true;
  clearReconnectTimer();
  client?.close();
  return languageServer?.stop();
}

module.exports = { activate, deactivate };
