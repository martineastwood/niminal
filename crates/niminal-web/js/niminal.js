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

  // ---- live: a session that runs for as long as there is sound, stepped a chunk at a time

  /** Run `call(pointer, length)` on `text` written into the module's memory. */
  #withText(text, call) {
    const wasm = this.#wasm;
    const encoded = new TextEncoder().encode(text);
    const pointer = wasm.niminal_alloc(encoded.length);
    new Uint8Array(wasm.memory.buffer, pointer, encoded.length).set(encoded);
    try {
      return call(pointer, encoded.length);
    } finally {
      wasm.niminal_free(pointer, encoded.length);
    }
  }

  #liveResult(status) {
    const text = JSON.parse(this.#text(this.#wasm.niminal_live_text(), this.#wasm.niminal_live_text_length()));
    return status === 0
      ? { ok: true, landed: text }
      : { ok: false, diagnostics: text.diagnostics, message: text.message, internal: false };
  }

  #trapped(error) {
    // a trap leaves the instance in an unknown state, and the session with it: start fresh
    const wasm = this.#wasm;
    const message = this.#text(wasm.niminal_panic_message(), wasm.niminal_panic_length()) || String(error);
    this.#start();
    return { ok: false, diagnostics: [], message, internal: true };
  }

  /** Start a session at `sampleRate` for `source` and run all of it now, replacing any session there was.
   *  `{ ok: true, landed: { bar, beat, changes } }` or `{ ok: false, diagnostics, message, internal }`. */
  liveStart(source, sampleRate) {
    try {
      return this.#liveResult(this.#withText(source, (p, n) => this.#wasm.niminal_live_start(p, n, sampleRate)));
    } catch (error) {
      return this.#trapped(error);
    }
  }

  /** Run `source` (a snippet) in the session. `when` says when it lands: "now", "beat", "bar", "next 4bars"...;
   *  without it each change lands where it asks. Offsets in `diagnostics` are within the snippet. */
  liveEval(source, when = "") {
    try {
      return this.#liveResult(this.#withText(source, (p, n) => this.#withText(when, (q, m) => this.#wasm.niminal_live_eval(p, n, q, m))));
    } catch (error) {
      return this.#trapped(error);
    }
  }

  /** The next `frames` samples: a Float32Array per channel (copies), or null with no session. */
  liveProcess(frames) {
    const wasm = this.#wasm;
    try {
      const channels = wasm.niminal_live_process(frames);
      if (channels === 0) return null;
      const out = [];
      for (let c = 0; c < channels; c++) {
        out.push(new Float32Array(wasm.memory.buffer, wasm.niminal_live_channel(c), frames).slice());
      }
      return out;
    } catch (error) {
      this.#trapped(error);
      return null;
    }
  }

  /** Where the music is and what is playing: `{ bar, beat, bpm, voices, pending, seconds, clips: [...] }`, or null. */
  liveStatus() {
    const wasm = this.#wasm;
    const length = wasm.niminal_live_status();
    return length === 0 ? null : JSON.parse(this.#text(wasm.niminal_live_text(), length));
  }

  /** Stop every sound and clip now, keeping the session. */
  liveHush() {
    this.#wasm.niminal_live_hush();
  }

  /** Move a control of the session now: `unit` is what the value is in ("hz", "db", ...), if it has one.
   *  `{ ok: true }` or `{ ok: false, message }`. */
  liveControl(name, value, unit = "") {
    try {
      const failed = this.#withText(`${name}|${unit}`, (p, n) => this.#wasm.niminal_live_control(p, n, value));
      return failed === 0 ? { ok: true } : { ok: false, message: this.#text(this.#wasm.niminal_live_text(), this.#wasm.niminal_live_text_length()) };
    } catch (error) {
      return this.#trapped(error);
    }
  }

  /** End the session. */
  liveStop() {
    this.#wasm.niminal_live_stop();
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
