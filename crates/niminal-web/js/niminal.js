// Loads niminal_web.wasm and renders pieces with it. Works in a browser, a Web Worker and Node.
//
//   import { load } from "./niminal.js";
//   const niminal = await load("niminal_web.wasm");     // a URL, or the bytes
//   const result = niminal.render(source, 8);          // 8 bars
//   result.ok ? result.channels /* Float32Array per channel */ : result.diagnostics
//   niminal.lsp({ jsonrpc: "2.0", id: 1, method: "initialize", params: {} })   // the language server, in answers
//
// The module imports one function, `env.niminal_now_ms`, because WebAssembly has no clock of its own.

const imports = { env: { niminal_now_ms: () => performance.now() } };

export async function load(source) {
  const bytes = typeof source === "string" || source instanceof URL
    ? await (await fetch(source)).arrayBuffer()
    : source;
  const module = await WebAssembly.compile(bytes);
  return new Niminal(module);
}

class Niminal {
  #module;
  #wasm;

  constructor(module) {
    this.#module = module;
    this.#start();
  }

  #start() {
    this.#wasm = new WebAssembly.Instance(this.#module, imports).exports;
    this.#wasm.niminal_init();
  }

  #text(pointer, length) {
    return new TextDecoder().decode(new Uint8Array(this.#wasm.memory.buffer, pointer, length));
  }

  /** Give the language server one LSP message (an object) and get back the messages it sends in answer. */
  lsp(message) {
    const wasm = this.#wasm;
    const encoded = new TextEncoder().encode(JSON.stringify(message));
    const pointer = wasm.niminal_alloc(encoded.length);
    new Uint8Array(wasm.memory.buffer, pointer, encoded.length).set(encoded);
    let length;
    try {
      length = wasm.niminal_lsp(pointer, encoded.length);
    } catch (error) {
      this.#start();
      throw error;
    }
    wasm.niminal_free(pointer, encoded.length);
    return JSON.parse(this.#text(wasm.niminal_lsp_reply(), length));
  }

  /** Render `source` for `bars` bars (0 or less: until it ends by itself).
   *  Returns `{ ok: true, sampleRate, channels, warnings }` or
   *  `{ ok: false, diagnostics: [{ start, end, message, help }], message, internal }`; offsets are in UTF-8 bytes. */
  render(source, bars = 8) {
    const wasm = this.#wasm;
    const encoded = new TextEncoder().encode(source);
    const pointer = wasm.niminal_alloc(encoded.length);
    new Uint8Array(wasm.memory.buffer, pointer, encoded.length).set(encoded);
    let status;
    try {
      status = wasm.niminal_render(pointer, encoded.length, bars);
    } catch (error) {
      // a trap leaves the instance in an unknown state: say what happened and start a fresh one
      const message = this.#text(wasm.niminal_panic_message(), wasm.niminal_panic_length()) || String(error);
      this.#start();
      return { ok: false, diagnostics: [], message, internal: true };
    }
    wasm.niminal_free(pointer, encoded.length);
    const report = JSON.parse(this.#text(wasm.niminal_report(), wasm.niminal_report_length()));
    if (status !== 0) {
      return { ok: false, diagnostics: report.diagnostics, message: report.message, internal: false };
    }
    const frames = wasm.niminal_frames();
    const channels = [];
    for (let c = 0; c < wasm.niminal_channels(); c++) {
      // copied out: the module reuses this memory for the next render
      channels.push(new Float32Array(wasm.memory.buffer, wasm.niminal_channel(c), frames).slice());
    }
    return { ok: true, sampleRate: wasm.niminal_sample_rate(), channels, warnings: report.warnings };
  }
}
