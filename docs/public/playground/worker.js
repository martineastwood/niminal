// Renders pieces and answers language-server requests off the page's thread (the page runs one worker for each, so typing is not held up by a render). The module is fetched on the first message (or on `warm`).
import { load } from "./niminal.js";

let niminal;

// ---- live: a session that this worker steps a little ahead of what the player has played
const CHUNK = 1024; // frames per step
const TARGET_SECONDS = 0.3; // how far ahead of the player to stay
let link; // the port to the player
let live; // { sampleRate, produced, played, load, underruns } while a session runs
let pumpTimer;
let statusTimer;

function pump() {
  if (!live) return;
  const target = TARGET_SECONDS * live.sampleRate;
  const began = performance.now();
  // a few steps at a time, so that a message (code to run) is never kept waiting long
  while (live.produced - live.played < target && performance.now() - began < 6) {
    const stepped = performance.now();
    const chunk = niminal.liveProcess(CHUNK);
    if (!chunk) { stopLive(); self.postMessage({ kind: "live-lost" }); return; }
    const cost = performance.now() - stepped;
    live.load = 0.9 * live.load + 0.1 * (cost / ((CHUNK / live.sampleRate) * 1000));
    for (const channel of chunk) for (let i = 0; i < channel.length; i += 8) live.peak = Math.max(live.peak, Math.abs(channel[i]));
    live.produced += CHUNK;
    link.postMessage({ chunk }, chunk.map((c) => c.buffer));
  }
}

function startLive(sampleRate) {
  if (live) return;
  live = { sampleRate, produced: 0, played: 0, load: 0, underruns: 0, peak: 0 };
  link.postMessage({ running: true });
  pumpTimer = setInterval(pump, 5);
  statusTimer = setInterval(() => {
    if (!live) return;
    self.postMessage({
      kind: "live-status",
      status: niminal.liveStatus(),
      load: live.load,
      peak: (() => { const p = live.peak; live.peak = 0; return p; })(),
      underruns: live.underruns,
      buffered: (live.produced - live.played) / live.sampleRate,
    });
  }, 250);
}

function stopLive() {
  clearInterval(pumpTimer);
  clearInterval(statusTimer);
  live = undefined;
  link?.postMessage({ running: false });
}

/** What is queued for the player is dropped, so a restart or a stop is heard at once. */
function clearQueue() {
  link?.postMessage({ clear: true });
}

self.onmessage = async (event) => {
  const { id, kind, source, bars } = event.data;
  try {
    niminal ??= await load(new URL("./niminal_web.wasm", import.meta.url));
    if (kind === "live-link") {
      link = event.data.link;
      link.onmessage = (message) => {
        if (!live) return;
        live.played = message.data.played;
        if (message.data.underrun) live.underruns++;
      };
      self.postMessage({ id, result: { ok: true } });
      return;
    }
    if (kind === "live-start") {
      const result = niminal.liveStart(source, event.data.sampleRate);
      if (result.ok) {
        // the new piece replaces the old: what was queued of the old one is dropped
        clearQueue();
        startLive(event.data.sampleRate);
      }
      self.postMessage({ id, result });
      return;
    }
    if (kind === "live-eval") {
      self.postMessage({ id, result: niminal.liveEval(source, event.data.now) });
      return;
    }
    if (kind === "live-control") {
      self.postMessage({ id, result: niminal.liveControl(event.data.name, event.data.value, event.data.unit) });
      return;
    }
    if (kind === "live-hush") {
      niminal.liveHush();
      clearQueue();
      self.postMessage({ id, result: { ok: true } });
      return;
    }
    if (kind === "live-stop") {
      stopLive();
      niminal.liveStop();
      clearQueue();
      self.postMessage({ id, result: { ok: true } });
      return;
    }
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
