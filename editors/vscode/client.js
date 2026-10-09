// A client for the niminal daemon's JSON-RPC protocol over a WebSocket.

const WebSocket = require("ws");

const PROTOCOL = 1;

class Client {
  /** `onNotification(method, params)` and `onClose()` are called as the daemon speaks or goes away. */
  constructor({ port, token, onNotification = () => {}, onClose = () => {} }) {
    this.port = port;
    this.token = token;
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
    const ws = new WebSocket(`ws://127.0.0.1:${this.port}/`);
    await new Promise((resolve, reject) => {
      ws.once("open", resolve);
      ws.once("error", () => reject(new Error(`can't reach a niminal daemon on port ${this.port}`)));
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
    const params = { protocol: PROTOCOL, client: "niminal-vscode" };
    if (this.token) params.token = this.token;
    await this.call("hello", params);
  }

  #receive(text) {
    const message = JSON.parse(text);
    if (message.id !== undefined && this.waiting.has(message.id)) {
      const { resolve, reject } = this.waiting.get(message.id);
      this.waiting.delete(message.id);
      if (message.error) {
        const error = new Error(message.error.message);
        error.problems = message.error.data?.problems;
        reject(error);
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

  close() {
    this.ws?.close();
  }
}

module.exports = { Client };
