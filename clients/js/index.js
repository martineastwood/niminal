// Generic daemon client. LiveClient controls a separate client-side live sequencer.

const WebSocket = globalThis.WebSocket ?? (typeof require === "function" ? require("ws") : undefined);

const PROTOCOL = 4;

class NiminalError extends Error {
  constructor(message, problems) {
    super(message);
    this.problems = problems;
  }
}

class Client {
  /** `onNotification(method, params)` and `onClose()` are called as the daemon speaks or goes away. */
  constructor({
    url,
    port = 7401,
    token,
    client = "niminal-js",
    onNotification = () => {},
    onClose = () => {},
  } = {}) {
    this.url = url ?? `ws://127.0.0.1:${port}/`;
    this.token = token;
    this.client = client;
    this.onNotification = onNotification;
    this.onClose = onClose;
    this.nextId = 1;
    this.waiting = new Map();
    this.ws = null;
  }

  get connected() {
    return this.ws !== null && this.ws.readyState === WebSocket.OPEN;
  }

  async connect() {
    const ws = new WebSocket(this.url);
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve, { once: true });
      ws.addEventListener("error", () => reject(new Error(`can't reach a niminal daemon at ${this.url}`)), { once: true });
    });
    this.ws = ws;
    ws.addEventListener("message", (event) => this.#receive(event.data.toString()));
    ws.addEventListener("close", () => {
      for (const { reject } of this.waiting.values()) reject(new Error("the daemon closed the connection"));
      this.waiting.clear();
      this.ws = null;
      this.onClose();
    });
    ws.addEventListener("error", () => {});
    const params = { protocol: PROTOCOL, client: this.client };
    if (this.token) params.token = this.token;
    return this.call("hello", params);
  }

  #receive(text) {
    const message = JSON.parse(text);
    if (message.id !== undefined && this.waiting.has(message.id)) {
      const { resolve, reject } = this.waiting.get(message.id);
      this.waiting.delete(message.id);
      if (message.error) {
        reject(new NiminalError(message.error.message, message.error.data?.problems));
      } else {
        resolve(message.result);
      }
    } else if (message.method) {
      this.onNotification(message.method, message.params);
    }
  }

  call(method, params = {}) {
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      this.waiting.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    });
  }

  status() {
    return this.call("status");
  }

  /**
   * Send timed events to the engine: `{ op: "note" | "control" | "track", at, ... }`.
   * `at` is an absolute sample count or "now". See the protocol reference.
   */
  sendEvents(events) {
    return this.call("events.send", { events });
  }

  /** Edit private processor instances, buses and connections at an absolute sample. */
  routing(edits, { at = "now", tag } = {}) {
    return this.sendEvents([{ op: "routing", at, edits, ...(tag === undefined ? {} : { tag }) }]);
  }

  /** Compile definitions and edit routing as a single scheduled graph transaction. */
  transaction({ source = "", edits = [], at = "now", dir, tag } = {}) {
    return this.sendEvents([{ op: "transaction", source, edits, at, ...(dir === undefined ? {} : {dir}), ...(tag === undefined ? {} : {tag}) }]);
  }

  /** Take back this client's waiting events: `{ tag, from, release }`. */
  cancelEvents({ tag, from, release } = {}) {
    const params = {};
    if (tag !== undefined) params.tag = tag;
    if (from !== undefined) params.from = from;
    if (release !== undefined) params.release = release;
    return this.call("events.cancel", params);
  }

  /** Where the engine is in time: sample, sample_rate, prepared_until, wall_micros... */
  clock() {
    return this.call("clock");
  }

  setControl(name, value, { unit, glide, at = "now" } = {}) {
    const op = { op: "control", name, value, at };
    if (unit !== undefined) op.unit = unit;
    if (glide !== undefined) op.glide = glide;
    return this.sendEvents([op]);
  }

  subscribe(topics) {
    return this.call("subscribe", { topics });
  }

  panic() { return this.call("panic"); }
  panicEngine() { return this.call("engine.panic"); }
  createScope(id, endpoint, { frames = 1024, rate = 20 } = {}) { return this.call("scopes.create", { id, endpoint, frames, rate }); }
  removeScope(id) { return this.call("scopes.remove", { id }); }
  bufferBudget(bytes) { return this.call("buffers.budget", bytes === undefined ? {} : { bytes }); }
  listBuffers() { return this.call("buffers.list"); }
  allocBuffer(name, frames, channels = 1) { return this.call("buffers.alloc", { name, frames, channels }); }
  freeBuffer(name) { return this.call("buffers.free", { name }); }
  loadBuffer(name, path, channel = 0) { return this.call("buffers.load", { name, path, channel }); }
  readBuffer(name, count, offset = 0) { return this.call("buffers.read", { name, count, offset }); }
  saveBuffer(name, path) { return this.call("buffers.save", { name, path }); }
  render(spec) { return this.call("render.start", spec); }
  jobStatus(id) { return this.call("jobs.status", { id }); }
  cancelJob(id) { return this.call("jobs.cancel", { id }); }
  capture(output, at = "now") { return this.call("capture.start", { output, at }); }
  stopCapture(id, at = "now") { return this.call("capture.stop", { id, at }); }
  audioDevices() { return this.call("audio.devices"); }
  audioStatus() { return this.call("audio.status"); }
  configureAudio(settings, { restart = false } = {}) { return this.call("audio.configure", { settings, restart }); }
  removeFile(id) { return this.call("files.remove", { id }); }

  /** Upload a browser File/Blob, ArrayBuffer, Uint8Array or Node Buffer. */
  async uploadFile(file) {
    const blob = typeof file.arrayBuffer === "function" && typeof file.slice === "function";
    const bytes = blob ? null : file instanceof ArrayBuffer ? new Uint8Array(file) : file;
    const size = blob ? file.size : bytes.byteLength;
    let id;
    for (let offset = 0; offset < size || id === undefined; offset += 262144) {
      const chunk = blob ? new Uint8Array(await file.slice(offset, offset + 262144).arrayBuffer()) : bytes.subarray(offset, offset + 262144);
      const data = typeof Buffer !== "undefined" ? Buffer.from(chunk).toString("base64") : btoa(Array.from(chunk, x => String.fromCharCode(x)).join(""));
      const result = await this.call("files.upload", { ...(id === undefined ? {} : { id }), offset, data, finish: offset + chunk.byteLength >= size });
      id = result.id;
    }
    return id;
  }

  /** Stream downloaded bytes to a callback; omit it to collect a Uint8Array. */
  async downloadFile(id, onChunk) {
    const chunks = [];
    let offset = 0;
    while (true) {
      const result = await this.call("files.read", { id, offset, count: 262144 });
      const chunk = typeof Buffer !== "undefined" ? new Uint8Array(Buffer.from(result.data, "base64")) : Uint8Array.from(atob(result.data), x => x.charCodeAt(0));
      if (onChunk) await onChunk(chunk); else chunks.push(chunk);
      offset += chunk.byteLength;
      if (result.eof) break;
    }
    if (onChunk) return;
    const data = new Uint8Array(offset);
    offset = 0;
    for (const chunk of chunks) { data.set(chunk, offset); offset += chunk.length; }
    return data;
  }

  /** Build a version-2 `.nms` project JSON string for offline render. */
  static projectToJson(project) {
    return JSON.stringify({ version: 2, ...project }, null, 2);
  }

  close() {
    this.ws?.close();
  }
}

/** Control a live sequencer started with `nimlive live`, rather than the daemon. */
class LiveClient extends Client {
  constructor(options = {}) {
    super({ port: 7400, ...options });
  }

  eval(source, { quantize, dir } = {}) {
    const params = { source };
    if (quantize) params.quantize = quantize;
    if (dir) params.dir = dir;
    return this.call("eval", params);
  }

  defineClip(clip, { quantize } = {}) {
    const params = { ...clip };
    if (quantize) params.quantize = quantize;
    return this.call("clip.define", params);
  }

  defineScene(scene, { quantize } = {}) {
    const params = { ...scene };
    if (quantize) params.quantize = quantize;
    return this.call("scene.define", params);
  }

  launch(target, { quantize, then } = {}) {
    const params = { target };
    if (quantize) params.quantize = quantize;
    if (then) params.then = then; // { target, after: "4bars" }
    return this.call("launch", params);
  }

  stop(target, { quantize } = {}) {
    return this.call("stop", { target, ...(quantize ? { quantize } : {}) });
  }

  mute(target, { quantize } = {}) {
    return this.call("mute", { target, ...(quantize ? { quantize } : {}) });
  }

  unmute(target, { quantize } = {}) {
    return this.call("unmute", { target, ...(quantize ? { quantize } : {}) });
  }

  solo(target, { quantize } = {}) {
    return this.call("solo", { target, ...(quantize ? { quantize } : {}) });
  }

  unsolo(target, { quantize } = {}) {
    const params = {};
    if (target) params.target = target;
    if (quantize) params.quantize = quantize;
    return this.call("unsolo", params);
  }

  edit(track, change, { quantize } = {}) {
    const params = { track, change };
    if (quantize) params.quantize = quantize;
    return this.call("edit", params);
  }

  hush() {
    return this.call("hush");
  }

  panic() {
    return this.call("panic");
  }

}

if (typeof module === "object" && module.exports) module.exports = { Client, LiveClient, NiminalError, PROTOCOL };
else globalThis.niminal = { Client, LiveClient, NiminalError, PROTOCOL };
