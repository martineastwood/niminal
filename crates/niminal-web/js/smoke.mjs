// node smoke.mjs niminal_web.wasm: the module loads, renders a piece, and reports a mistake where it is.
import { readFileSync } from "node:fs";
import { load } from "./niminal.js";

const niminal = await load(readFileSync(process.argv[2]));
const fail = (message) => { console.error(`FAIL: ${message}`); process.exit(1); };

const piece = `tempo 120bpm
instr tone(freq: hz) { out = osc(sine, freq) * env[0 2ms 1 | 100ms 0] }
clip c {
  instrument: tone
  length: 1bar
  notes: [a3 ~ c4 ~]
}
c.start
`;
const good = niminal.render(piece, 2);
if (!good.ok) fail(`a piece did not render: ${JSON.stringify(good)}`);
if (good.sampleRate !== 48000 || good.channels.length < 1) fail("unexpected format");
if (good.channels[0].length < 2 * 48000) fail("too short");
if (!good.channels[0].some((s) => Math.abs(s) > 0.01)) fail("silent");

const bad = niminal.render("instr a() { out = nonsense(1) }\n", 1);
if (bad.ok || bad.diagnostics.length === 0) fail("a mistake was not reported");
if (!(bad.diagnostics[0].start >= 0 && bad.diagnostics[0].end > bad.diagnostics[0].start)) fail("no location");

const again = niminal.render(piece, 2);
if (!again.ok || again.channels[0].length !== good.channels[0].length) fail("a second render differs");
// the language server answers
const init = niminal.lsp({ jsonrpc: "2.0", id: 1, method: "initialize", params: { capabilities: {} } });
if (!init.some((m) => m.id === 1 && m.result?.capabilities)) fail(`initialize: ${JSON.stringify(init)}`);
niminal.lsp({ jsonrpc: "2.0", method: "initialized", params: {} });
const uri = "file:///playground.nml";
const text = "instr a() { out = nonsense(1) }\ninstr b() { out = os }\ninstr c() { out = osc(sine, 220hz) }\n";
const opened = niminal.lsp({ jsonrpc: "2.0", method: "textDocument/didOpen", params: { textDocument: { uri, languageId: "niminal", version: 1, text } } });
const published = opened.find((m) => m.method === "textDocument/publishDiagnostics");
if (!published || published.params.diagnostics.length === 0) fail(`no diagnostics: ${JSON.stringify(opened)}`);
const completion = niminal.lsp({ jsonrpc: "2.0", id: 2, method: "textDocument/completion", params: { textDocument: { uri }, position: { line: 1, character: 21 } } });
const items = completion.find((m) => m.id === 2)?.result;
const labels = (Array.isArray(items) ? items : items?.items ?? []).map((i) => i.label);
if (!labels.includes("osc")) fail(`no completion for "os": ${labels.slice(0, 8)}`);
const hover = niminal.lsp({ jsonrpc: "2.0", id: 3, method: "textDocument/hover", params: { textDocument: { uri }, position: { line: 2, character: 20 } } });
const hovered = hover.find((m) => m.id === 3)?.result;
if (!hovered) fail("no hover on `osc`");
console.log(`language server: ${published.params.diagnostics.length} diagnostic(s), ${labels.length} completions, hover answers`);
console.log(`ok: ${good.channels.length} channels, ${(good.channels[0].length / 48000).toFixed(2)} s, error at bytes ${bad.diagnostics[0].start}-${bad.diagnostics[0].end}`);
