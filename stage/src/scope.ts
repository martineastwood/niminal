import { ScopeFrame, stage } from './live';

export type ScopeMode = 'off' | 'wave' | 'spec' | 'both';

const MODES: ScopeMode[] = ['off', 'wave', 'spec', 'both'];
const STORAGE_KEY = 'niminal-stage-scope';
const DEFAULT_FFT = 1024;
const MIN_DB = -84;
const MIN_HZ = 20;
const MAX_HZ = 20000;
const POLL_MS = 33;

/** Average interleaved multichannel audio into mono. */
export function downmixMono(samples: ArrayLike<number>, channels: number): Float32Array {
  const ch = Math.max(1, channels);
  const frames = Math.floor(samples.length / ch);
  const out = new Float32Array(frames);
  if (ch === 1) {
    for (let i = 0; i < frames; i++) out[i] = samples[i];
    return out;
  }
  const scale = 1 / ch;
  for (let i = 0; i < frames; i++) {
    let sum = 0;
    const base = i * ch;
    for (let c = 0; c < ch; c++) sum += samples[base + c];
    out[i] = sum * scale;
  }
  return out;
}

/** Extract a single channel from interleaved audio. */
export function extractChannel(samples: ArrayLike<number>, channels: number, channel: number): Float32Array {
  const ch = Math.max(1, channels);
  const c = Math.min(Math.max(0, channel), ch - 1);
  const frames = Math.floor(samples.length / ch);
  const out = new Float32Array(frames);
  for (let i = 0; i < frames; i++) out[i] = samples[i * ch + c];
  return out;
}

/** Find a rising zero-crossing so periodic waveforms stay steady on screen. */
export function triggerIndex(samples: ArrayLike<number>, windowSize: number): number {
  const maxStart = samples.length - windowSize;
  if (maxStart <= 1) return 0;
  let best = -1;
  let bestSlope = 0;
  for (let i = 1; i <= maxStart; i++) {
    const prev = samples[i - 1];
    const curr = samples[i];
    if (prev <= 0 && curr > 0) {
      const slope = curr - prev;
      if (slope > bestSlope) {
        bestSlope = slope;
        best = i;
      }
    }
  }
  return best >= 0 ? best : 0;
}

/** Radix-2 Cooley-Tukey FFT on real input (Hann-windowed), returning magnitude in dB (`[MIN_DB, 0]`) for `fftSize/2 + 1` bins. */
export function computeSpectrum(samples: ArrayLike<number>, fftSize = DEFAULT_FFT): Float32Array {
  const n = fftSize;
  const re = new Float32Array(n);
  const im = new Float32Array(n);
  const start = Math.max(0, samples.length - n);
  const count = Math.min(samples.length, n);
  const offset = n - count;
  const invN = 1 / (n - 1);
  for (let i = 0; i < count; i++) {
    const idx = offset + i;
    const w = 0.5 * (1 - Math.cos(2 * Math.PI * idx * invN));
    re[idx] = samples[start + i] * w;
  }

  // bit-reversal permutation
  for (let i = 1, j = 0; i < n; i++) {
    let bit = n >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) {
      const tr = re[i];
      re[i] = re[j];
      re[j] = tr;
    }
  }

  // iterative butterflies
  for (let len = 2; len <= n; len <<= 1) {
    const half = len >> 1;
    const step = (-2 * Math.PI) / len;
    const wReStep = Math.cos(step);
    const wImStep = Math.sin(step);
    for (let i = 0; i < n; i += len) {
      let wRe = 1;
      let wIm = 0;
      for (let j = 0; j < half; j++) {
        const u = i + j;
        const v = u + half;
        const tRe = re[v] * wRe - im[v] * wIm;
        const tIm = re[v] * wIm + im[v] * wRe;
        re[v] = re[u] - tRe;
        im[v] = im[u] - tIm;
        re[u] += tRe;
        im[u] += tIm;
        const nextRe = wRe * wReStep - wIm * wImStep;
        wIm = wRe * wImStep + wIm * wReStep;
        wRe = nextRe;
      }
    }
  }

  // Hann coherent gain is 0.5, so full-scale sine yields amplitude n / 4
  const bins = (n >> 1) + 1;
  const norm = 4 / n;
  const db = new Float32Array(bins);
  for (let k = 0; k < bins; k++) {
    const mag = Math.hypot(re[k], im[k]) * norm;
    const val = mag > 1e-9 ? 20 * Math.log10(mag) : MIN_DB;
    db[k] = Math.max(MIN_DB, Math.min(0, val));
  }
  return db;
}

/** Map a horizontal pixel coordinate `x` in `[0, width]` onto a logarithmic frequency in Hz. */
export function freqForX(x: number, width: number, sampleRate: number): number {
  const maxHz = Math.min(MAX_HZ, sampleRate * 0.5);
  const t = Math.min(Math.max(x / Math.max(1, width), 0), 1);
  return MIN_HZ * Math.pow(maxHz / MIN_HZ, t);
}

/** Map a frequency in Hz onto a horizontal pixel coordinate in `[0, width]`. */
export function xForFreq(hz: number, width: number, sampleRate: number): number {
  const maxHz = Math.min(MAX_HZ, sampleRate * 0.5);
  if (hz <= MIN_HZ || maxHz <= MIN_HZ) return 0;
  return (Math.log(Math.min(hz, maxHz) / MIN_HZ) / Math.log(maxHz / MIN_HZ)) * width;
}

function loadMode(): ScopeMode {
  const raw = localStorage.getItem(STORAGE_KEY) as ScopeMode | null;
  return raw && MODES.includes(raw) ? raw : 'off';
}

export function createScope(host: HTMLElement) {
  let mode: ScopeMode = loadMode();
  let connected = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let inFlight = false;
  let smoothed: Float32Array | null = null;

  host.innerHTML = `
    <div class="scope-head">
      <span class="scope-title">scope</span>
      <div class="scope-modes" role="group" aria-label="Scope mode">
        <button data-mode="off" title="Hide oscilloscope and spectrum (zero audio overhead)">off</button>
        <button data-mode="wave" title="Oscilloscope">wave</button>
        <button data-mode="spec" title="Spectrum analyzer">spec</button>
        <button data-mode="both" title="Oscilloscope and spectrum">both</button>
      </div>
    </div>
    <div class="scope-body" hidden>
      <canvas class="scope-canvas" aria-label="Audio oscilloscope and spectrum"></canvas>
    </div>
  `;

  const body = host.querySelector<HTMLDivElement>('.scope-body')!;
  const canvas = host.querySelector<HTMLCanvasElement>('.scope-canvas')!;
  const buttons = Array.from(host.querySelectorAll<HTMLButtonElement>('.scope-modes button'));

  function syncBackend() {
    if (!connected) return;
    void stage.scopeSet(mode !== 'off').catch(() => {});
  }

  function applyMode(next: ScopeMode, save = true) {
    mode = next;
    if (save) localStorage.setItem(STORAGE_KEY, mode);
    const active = mode !== 'off';
    body.hidden = !active;
    host.classList.toggle('open', active);
    for (const btn of buttons) btn.classList.toggle('active', btn.dataset.mode === mode);
    smoothed = null;
    syncBackend();
    if (active) {
      drawFrame(null);
      schedule();
    } else if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  }

  for (const btn of buttons) {
    btn.addEventListener('click', () => {
      const m = btn.dataset.mode as ScopeMode;
      if (MODES.includes(m)) applyMode(m);
    });
  }

  function resizeCanvas(): { ctx: CanvasRenderingContext2D; w: number; h: number } | null {
    const ctx = canvas.getContext('2d');
    if (!ctx) return null;
    const dpr = window.devicePixelRatio || 1;
    const rect = canvas.getBoundingClientRect();
    const w = Math.max(1, Math.round(rect.width || 280));
    const h = Math.max(1, Math.round(rect.height || 88));
    if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    return { ctx, w, h };
  }

  function drawGrid(ctx: CanvasRenderingContext2D, w: number, h: number, sampleRate: number) {
    ctx.fillStyle = '#0f1113';
    ctx.fillRect(0, 0, w, h);

    ctx.strokeStyle = 'rgba(110, 117, 127, 0.16)';
    ctx.lineWidth = 1;
    ctx.beginPath();
    if (mode === 'wave' || mode === 'both') {
      const mid = Math.round(h / 2) + 0.5;
      ctx.moveTo(0, mid);
      ctx.lineTo(w, mid);
    }
    if (mode === 'spec' || mode === 'both') {
      for (const hz of [100, 1000, 10000]) {
        const x = Math.round(xForFreq(hz, w, sampleRate)) + 0.5;
        ctx.moveTo(x, 0);
        ctx.lineTo(x, h);
      }
      for (const db of [-12, -36, -60]) {
        const y = Math.round(((0 - db) / (0 - MIN_DB)) * h) + 0.5;
        ctx.moveTo(0, y);
        ctx.lineTo(w, y);
      }
    }
    ctx.stroke();

    if (mode === 'spec' || mode === 'both') {
      ctx.fillStyle = 'rgba(110, 117, 127, 0.65)';
      ctx.font = '9px ui-monospace, Menlo, monospace';
      ctx.textBaseline = 'bottom';
      for (const [hz, label] of [
        [100, '100'],
        [1000, '1k'],
        [10000, '10k'],
      ] as const) {
        const x = xForFreq(hz, w, sampleRate);
        ctx.fillText(label, x + 2, h - 2);
      }
    }
  }

  function drawSpectrum(ctx: CanvasRenderingContext2D, w: number, h: number, mono: Float32Array, sampleRate: number) {
    const db = computeSpectrum(mono, DEFAULT_FFT);
    if (!smoothed || smoothed.length !== db.length) {
      smoothed = new Float32Array(db);
    } else {
      for (let i = 0; i < db.length; i++) {
        // fast attack, smooth release
        smoothed[i] = db[i] > smoothed[i] ? smoothed[i] * 0.35 + db[i] * 0.65 : smoothed[i] * 0.78 + db[i] * 0.22;
      }
    }
    const bins = smoothed.length;
    const nyquist = sampleRate * 0.5;
    const step = Math.max(1, Math.floor(w / 180));

    ctx.beginPath();
    ctx.moveTo(0, h);
    for (let x = 0; x <= w; x += step) {
      const hz = freqForX(x, w, sampleRate);
      const bin = Math.min(bins - 1, Math.max(1, Math.round((hz / nyquist) * (bins - 1))));
      const norm = (smoothed[bin] - MIN_DB) / (0 - MIN_DB);
      const y = h - Math.max(0, Math.min(1, norm)) * (h - 4) - 1;
      ctx.lineTo(x, y);
    }
    ctx.lineTo(w, h);
    ctx.closePath();
    ctx.fillStyle = mode === 'both' ? 'rgba(140, 188, 231, 0.16)' : 'rgba(217, 144, 63, 0.22)';
    ctx.fill();

    ctx.strokeStyle = mode === 'both' ? 'rgba(140, 188, 231, 0.65)' : '#d9903f';
    ctx.lineWidth = 1.25;
    ctx.stroke();
  }

  function drawWaveform(ctx: CanvasRenderingContext2D, w: number, h: number, frame: ScopeFrame) {
    const mono = downmixMono(frame.samples, frame.channels);
    const windowSize = Math.min(512, mono.length);
    if (windowSize < 2) return;
    const start = triggerIndex(mono, windowSize);
    const mid = h * 0.5;
    const amp = h * 0.44;

    const plot = (data: Float32Array, color: string, width: number) => {
      ctx.beginPath();
      for (let i = 0; i < windowSize; i++) {
        const x = (i / (windowSize - 1)) * w;
        const s = Math.max(-1.05, Math.min(1.05, data[start + i] ?? 0));
        const y = mid - s * amp;
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.strokeStyle = color;
      ctx.lineWidth = width;
      ctx.stroke();
    };

    if (frame.channels >= 2) {
      const right = extractChannel(frame.samples, frame.channels, 1);
      const left = extractChannel(frame.samples, frame.channels, 0);
      plot(right, 'rgba(140, 188, 231, 0.6)', 1);
      plot(left, '#f2c58e', 1.4);
    } else {
      plot(mono, '#f2c58e', 1.4);
    }
  }

  function drawFrame(frame: ScopeFrame | null) {
    const surface = resizeCanvas();
    if (!surface) return;
    const { ctx, w, h } = surface;
    const rate = frame?.sample_rate || 48000;
    drawGrid(ctx, w, h, rate);
    if (!frame || frame.samples.length === 0) return;

    if (mode === 'spec' || mode === 'both') {
      drawSpectrum(ctx, w, h, downmixMono(frame.samples, frame.channels), rate);
    }
    if (mode === 'wave' || mode === 'both') {
      drawWaveform(ctx, w, h, frame);
    }
  }

  function schedule() {
    if (timer !== null || mode === 'off') return;
    timer = setTimeout(tick, POLL_MS);
  }

  async function tick() {
    timer = null;
    if (mode === 'off') return;
    if (!connected || document.hidden) {
      timer = setTimeout(tick, 250);
      return;
    }
    if (!inFlight) {
      inFlight = true;
      try {
        const frame = await stage.scopeFrame(DEFAULT_FFT);
        drawFrame(frame);
      } catch {
        // non-Tauri or engine restarting
      } finally {
        inFlight = false;
      }
    }
    schedule();
  }

  function setConnected(ok: boolean) {
    if (connected === ok) return;
    connected = ok;
    if (ok) {
      syncBackend();
      if (mode !== 'off') schedule();
    } else if (mode !== 'off') {
      drawFrame(null);
    }
  }

  applyMode(mode, false);

  return { setConnected, setMode: (next: ScopeMode) => applyMode(next) };
}
