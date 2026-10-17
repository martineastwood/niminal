// Generic daemon client. LiveClient controls a separate client-side live sequencer.

const WebSocket = require("ws");

const PROTOCOL = 1;

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
      ws.once("open", resolve);
      ws.once("error", () => reject(new Error(`can't reach a niminal daemon at ${this.url}`)));
    });
    this.ws = ws;
    ws.on("message", (data) => this.#receive(data.toString()));
    ws.on("close", () => {
      for (const { reject } of this.waiting.values()) reject(new Error("the daemon closed the connection"));
      this.waiting.clear();
      this.ws = null;
      this.onClose();
    });
    ws.on("error", () => {});
    const params = { protocol: PROTOCOL, client: this.client };
    if (this.token) params.token = this.token;
    await this.call("hello", params);
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
   * `at` is a sample, "now" or a position such as "32beat". See the protocol reference.
   */
  sendEvents(events) {
    return this.call("events.send", { events });
  }

  /** Take back this client's waiting events: `{ tag, from, release }`. */
  cancelEvents({ tag, from, release } = {}) {
    const params = {};
    if (tag !== undefined) params.tag = tag;
    if (from !== undefined) params.from = from;
    if (release !== undefined) params.release = release;
    return this.call("events.cancel", params);
  }

  /** Where the engine is in time: sample, bar, beat, bpm, prepared_until, wall_micros... */
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

  /** Build a version-2 `.nms` project JSON string for offline render. */
  static projectToJson(project) {
    return JSON.stringify({ version: 2, ...project }, null, 2);
  }

  close() {
    this.ws?.close();
  }
}

/** Control a live sequencer started with `niminal live`, rather than the daemon. */
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

module.exports = { Client, LiveClient, NiminalError, PROTOCOL };
