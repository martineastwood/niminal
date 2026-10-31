import { describe, expect, it } from 'vitest';
import { auditionClip, faderFor, format, positionOf, valueAt } from './lab';

describe('lab faders', () => {
  it('spans a ranged parameter, and skips what a keyboard or a performer plays', () => {
    expect(faderFor({ name: 'freq', unit: 'hz', default: null, range: null })).toBeNull();
    expect(faderFor({ name: 'gain', unit: 'db', default: 1, range: null })).toBeNull();
    expect(faderFor({ name: 'bend', unit: 'st', default: 0, range: [-12, 12] })).toBeNull();
    const density = faderFor({ name: 'density', unit: 'hz', default: 30, range: [5, 100] })!;
    expect([density.min, density.max, density.start, density.log]).toEqual([5, 100, 30, true]);
  });

  it('reads db, which the engine reports as gain, back as db', () => {
    const amp = faderFor({ name: 'amp', unit: 'db', default: 0.5011872, range: [0.001, 1] })!;
    expect(amp.min).toBeCloseTo(-60);
    expect(amp.max).toBeCloseTo(0);
    expect(amp.start).toBeCloseTo(-6, 1);
    expect(amp.log).toBe(false);
  });

  it('maps positions to values and back, evenly in hz and sec', () => {
    const f = faderFor({ name: 'density', unit: 'hz', default: null, range: [10, 1000] })!;
    expect(valueAt(f, 0)).toBeCloseTo(10);
    expect(valueAt(f, 0.5)).toBeCloseTo(100);
    expect(valueAt(f, 1)).toBeCloseTo(1000);
    expect(positionOf(f, 100)).toBeCloseTo(0.5);
    const w = faderFor({ name: 'w', unit: '', default: 0.25, range: [0, 1] })!;
    expect(valueAt(w, 0.25)).toBeCloseTo(0.25);
    expect(w.start).toBe(0.25);
  });

  it('writes values the way code does', () => {
    expect(format('hz', 1200)).toBe('1.20khz');
    expect(format('sec', 0.25)).toBe('250ms');
    expect(format('db', -6)).toBe('-6.0db');
    expect(format('', 0.3)).toBe('0.30');
  });

  it('plays an instrument through a clip held keys sound', () => {
    expect(auditionClip('cello')).toContain('instrument: cello');
    expect(auditionClip('cello')).toContain('midi.keys');
  });
});
