// Renders pieces and answers language-server requests off the page's thread (the page runs one worker for each, so typing is not held up by a render). The module is fetched on the first message (or on `warm`).
import { load } from "./niminal.js";

let niminal;

self.onmessage = async (event) => {
  const { id, kind, source, bars } = event.data;
  try {
    niminal ??= await load(new URL("./niminal_web.wasm", import.meta.url));
    if (kind === "warm") {
      self.postMessage({ id, result: { ok: true, warmed: true } });
      return;
    }
    if (kind === "lsp") {
      self.postMessage({ id, replies: niminal.lsp(event.data.message) });
      return;
    }
    const started = performance.now();
    const result = niminal.render(source, bars);
    result.renderMs = performance.now() - started;
    self.postMessage({ id, result }, result.ok ? result.channels.map((c) => c.buffer) : []);
  } catch (error) {
    self.postMessage({ id, result: { ok: false, diagnostics: [], message: String(error), internal: true } });
  }
};
