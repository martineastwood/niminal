// The engine as the web pages use it: the workers, the audio path and the live session. No page
// markup in here, so the examples page and the workbench build on the same thing.

export type Diagnostic = { start: number; end: number; message: string; help?: string | null };
export type RenderResult =
  | { ok: true; sampleRate: number; channels: Float32Array[]; warnings: string[]; renderMs: number }
  | { ok: false; diagnostics: Diagnostic[]; message: string; internal: boolean };
export type Landed = { bar: number; beat: number; changes: string[] };
export type LiveReply =
  | { ok: true; landed: Landed }
  | { ok: false; diagnostics: Diagnostic[]; message: string; internal: boolean };
export type Status = {
  buffered: number;
  /** the loudest sample made since the last status */
  peak: number;
  load: number;
  underruns: number;
  status: { bar: number; beat: number; bpm: number; voices: number; clips: { name: string; running: boolean; muted: boolean }[] } | null;
};

export type Engine = ReturnType<typeof createEngine>;

export function createEngine(hooks: {
  /** how the running session is doing, a few times a second */
  onStatus: (status: Status) => void;
  /** the session ended without being asked to */
  onLost: () => void;
  /** the engine could not be loaded at all */
  onBroken: () => void;
}) {
  let nextId = 1;
  let sessionOn = false;
  let ctx: AudioContext | undefined;
  let audioReady: Promise<void> | undefined;

  // the engine runs in a worker, so the page stays responsive
  const worker = new Worker('/web/worker.js', { type: 'module' });
  const pending = new Map<number, (r: any) => void>();
  worker.onmessage = (e) => {
    // the live session talks without being asked: how it is doing, and if it was lost
    if (e.data.kind === 'live-status') return hooks.onStatus(e.data);
    if (e.data.kind === 'live-lost') { sessionOn = false; hooks.onLost(); return; }
    pending.get(e.data.id)?.(e.data.result);
    pending.delete(e.data.id);
  };
  worker.onerror = () => hooks.onBroken();
  const ask = <T>(message: object, transfer: Transferable[] = []) => new Promise<T>((resolve) => {
    const id = nextId++;
    pending.set(id, resolve);
    worker.postMessage({ id, ...message }, transfer);
  });

  // the language server runs in a worker of its own, so typing is not held up by a render or a running piece
  const lspWorker = new Worker('/web/worker.js', { type: 'module' });
  const lspPending = new Map<number, (replies: Record<string, unknown>[]) => void>();
  lspWorker.onmessage = (e) => { lspPending.get(e.data.id)?.(e.data.replies ?? []); lspPending.delete(e.data.id); };
  const exchange = (message: Record<string, unknown>) => new Promise<Record<string, unknown>[]>((resolve) => {
    const id = nextId++;
    lspPending.set(id, resolve);
    lspWorker.postMessage({ id, kind: 'lsp', message });
  });

  /** The audio context. Make it inside a click: a browser only allows it there. */
  function context() {
    ctx ??= new AudioContext();
    void ctx.resume();
    return ctx;
  }

  /** Make the audio path (context, player, link to the worker). Call it inside a click, and wait for it. */
  function ensureAudio() {
    const c = context();
    audioReady ??= (async () => {
      await c.audioWorklet.addModule('/web/playback-worklet.js');
      const node = new AudioWorkletNode(c, 'niminal-player', { outputChannelCount: [2] });
      node.connect(c.destination);
      const channel = new MessageChannel();
      node.port.postMessage({ link: channel.port1 }, [channel.port1]);
      await ask({ kind: 'live-link', link: channel.port2 }, [channel.port2]);
    })().catch((error) => { audioReady = undefined; throw error; });
    return audioReady;
  }

  return {
    exchange,
    context,
    ensureAudio,
    get sessionOn() { return sessionOn; },
    /** Load the engine in the background so the first run is quick. */
    warm: () => ask({ kind: 'warm' }),
    /** Render a whole piece offline. */
    render: (source: string, bars: number) => ask<RenderResult>({ kind: 'render', source, bars }),
    /** Start the piece as a live session (replacing any), all of it, now. */
    async start(source: string): Promise<LiveReply> {
      await ensureAudio();
      const reply = await ask<LiveReply>({ kind: 'live-start', source, sampleRate: context().sampleRate });
      if (reply.ok) sessionOn = true;
      return reply;
    },
    /** Run a piece of code in the session. Positions in a failure are within `source`. */
    run(source: string, now = false) {
      void ctx?.resume();
      return ask<LiveReply>({ kind: 'live-eval', source, now });
    },
    /** Move a control of the piece now. `unit` is what the value is in (`hz`, `db`), if anything. */
    control: (name: string, value: number, unit = '') =>
      ask<{ ok: true } | { ok: false; message: string }>({ kind: 'live-control', name, value, unit }),
    /** Silence everything, keeping the session. */
    async hush() { if (sessionOn) await ask({ kind: 'live-hush' }); },
    /** End the session. */
    async stop() {
      if (!sessionOn) return;
      sessionOn = false;
      await ask({ kind: 'live-stop' });
    },
  };
}

// ---- small helpers the pages share

/** The engine reports positions in UTF-8 bytes; the editor counts UTF-16 units. */
export function byteToIndex(text: string, byte: number) {
  let bytes = 0;
  for (let i = 0; i < text.length; ) {
    if (bytes >= byte) return i;
    const point = text.codePointAt(i)!;
    bytes += point < 0x80 ? 1 : point < 0x800 ? 2 : point < 0x10000 ? 3 : 4;
    i += point > 0xffff ? 2 : 1;
  }
  return text.length;
}

export function lineColumn(text: string, index: number) {
  const before = text.slice(0, index);
  return { line: before.split('\n').length, column: index - before.lastIndexOf('\n') };
}

/** Diagnostics located within a piece of the text, moved to where the piece sits in the whole. */
export function shiftDiagnostics(text: string, from: number, diagnostics: Diagnostic[]) {
  const base = new TextEncoder().encode(text.slice(0, from)).length;
  return diagnostics.map((d) => ({ ...d, start: d.start + base, end: d.end + base }));
}

/** A 16-bit WAV file of the channels. */
export function wavFile(channels: Float32Array[], sampleRate: number) {
  const frames = channels[0].length;
  const data = new DataView(new ArrayBuffer(44 + frames * channels.length * 2));
  const text = (at: number, s: string) => [...s].forEach((c, i) => data.setUint8(at + i, c.charCodeAt(0)));
  text(0, 'RIFF'); data.setUint32(4, 36 + frames * channels.length * 2, true); text(8, 'WAVEfmt ');
  data.setUint32(16, 16, true); data.setUint16(20, 1, true); data.setUint16(22, channels.length, true);
  data.setUint32(24, sampleRate, true); data.setUint32(28, sampleRate * channels.length * 2, true);
  data.setUint16(32, channels.length * 2, true); data.setUint16(34, 16, true);
  text(36, 'data'); data.setUint32(40, frames * channels.length * 2, true);
  let at = 44;
  for (let i = 0; i < frames; i++) {
    for (const c of channels) {
      data.setInt16(at, Math.round(Math.max(-1, Math.min(1, c[i])) * 32767), true);
      at += 2;
    }
  }
  return new Blob([data], { type: 'audio/wav' });
}

// ---- sharing: the piece, compressed, in the address
const toBase64Url = (bytes: Uint8Array) =>
  btoa(String.fromCharCode(...bytes)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
const fromBase64Url = (s: string) =>
  Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0));
async function pipe(bytes: Uint8Array, stream: CompressionStream | DecompressionStream) {
  const writer = stream.writable.getWriter();
  void writer.write(bytes as BufferSource);
  void writer.close();
  return new Uint8Array(await new Response(stream.readable).arrayBuffer());
}
export const encodePiece = async (text: string) => toBase64Url(await pipe(new TextEncoder().encode(text), new CompressionStream('deflate-raw')));
export const decodePiece = async (s: string) => new TextDecoder().decode(await pipe(fromBase64Url(s), new DecompressionStream('deflate-raw')));
