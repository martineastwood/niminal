// A small example client for the niminal server: connect, send timed events, read the clock.
// Everything else the server offers is reachable with `call(method, params)`; see the Engine API reference.

const WebSocket = globalThis.WebSocket ?? (typeof require === "function" ? require("ws") : undefined);

const PROTOCOL = 5;

class NiminalError extends Error {
  constructor(message, problems) {
    super(message);
    this.problems = problems;
  }
}

class Client {
  /** `onNotification(method, params)` and `onClose()` are called as the server speaks or goes away. */
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
      ws.addEventListener("error", () => reject(new Error(`can't reach a niminal server at ${this.url}`)), { once: true });
    });
    this.ws = ws;
    ws.addEventListener("message", (event) => this.#receive(event.data.toString()));
    ws.addEventListener("close", () => {
      for (const { reject } of this.waiting.values()) reject(new Error("the server closed the connection"));
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
   * Send timed events to the engine: `{ op: "note" | "control" | "routing", at, ... }`.
   * `at` is an absolute sample count or "now". See the protocol reference.
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

  /** Where the engine is in time: sample, sample_rate, prepared_until, wall_micros... */
  clock() {
    return this.call("clock");
  }

  subscribe(topics) {
    return this.call("subscribe", { topics });
  }

  panic() {
    return this.call("panic");
  }

  close() {
    this.ws?.close();
  }
}

if (typeof module === "object" && module.exports) module.exports = { Client, NiminalError, PROTOCOL };
else globalThis.niminal = { Client, NiminalError, PROTOCOL };
