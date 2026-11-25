// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import { computeSpectrum, createScope, downmixMono, extractChannel, freqForX, triggerIndex, xForFreq } from './scope';

vi.mock('./live', () => ({
  stage: {
    scopeSet: vi.fn().mockResolvedValue(undefined),
    scopeFrame: vi.fn().mockResolvedValue(null),
  },
}));

describe('scope DSP helpers', () => {
  it('downmixes interleaved stereo to mono and extracts individual channels', () => {
    const interleaved = [1, -1, 0.5, 0.5, -0.25, 0.75];
    expect(Array.from(downmixMono(interleaved, 2))).toEqual([0, 0.5, 0.25]);
    expect(Array.from(extractChannel(interleaved, 2, 0))).toEqual([1, 0.5, -0.25]);
    expect(Array.from(extractChannel(interleaved, 2, 1))).toEqual([-1, 0.5, 0.75]);
  });

  it('locks onto a rising zero-crossing', () => {
    const n = 256;
    const wave = new Float32Array(n);
    for (let i = 0; i < n; i++) wave[i] = Math.sin((2 * Math.PI * i) / 32 + 1.2);
    const idx = triggerIndex(wave, 128);
    expect(idx).toBeGreaterThan(0);
    expect(wave[idx - 1]).toBeLessThanOrEqual(0);
    expect(wave[idx]).toBeGreaterThan(0);
  });

  it('finds the peak bin of a sine wave in the FFT spectrum', () => {
    const sampleRate = 48000;
    const fftSize = 1024;
    const targetBin = 24; // 1125 Hz
    const freq = (targetBin * sampleRate) / fftSize;
    const samples = new Float32Array(fftSize);
    for (let i = 0; i < fftSize; i++) samples[i] = Math.sin((2 * Math.PI * freq * i) / sampleRate);

    const db = computeSpectrum(samples, fftSize);
    expect(db.length).toBe(fftSize / 2 + 1);
    let peakBin = 0;
    for (let i = 1; i < db.length; i++) if (db[i] > db[peakBin]) peakBin = i;
    expect(peakBin).toBe(targetBin);
    expect(db[peakBin]).toBeGreaterThan(-1);
    expect(db[peakBin + 20]).toBeLessThan(-50);
  });

  it('maps frequencies logarithmically and round-trips x <-> hz', () => {
    const width = 400;
    const sr = 48000;
    for (const hz of [20, 100, 1000, 10000, 20000]) {
      const x = xForFreq(hz, width, sr);
      expect(freqForX(x, width, sr)).toBeCloseTo(hz, 2);
    }
  });
});

describe('createScope UI', () => {
  it('defaults to off, toggles modes, and persists selection', () => {
    localStorage.clear();
    const host = document.createElement('div');
    document.body.append(host);
    const scope = createScope(host);
    const body = host.querySelector<HTMLDivElement>('.scope-body')!;
    expect(body.hidden).toBe(true);

    host.querySelector<HTMLButtonElement>('button[data-mode="both"]')!.click();
    expect(body.hidden).toBe(false);
    expect(localStorage.getItem('niminal-stage-scope')).toBe('both');

    scope.setMode('off');
    expect(body.hidden).toBe(true);
    expect(localStorage.getItem('niminal-stage-scope')).toBe('off');
    host.remove();
  });
});
