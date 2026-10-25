import { expect, it } from 'vitest';
import { readLfos, setLfo } from './modulation';

const cutoff = { name: 'cutoff', min: 200, max: 4000, default: 600, unit: 'hz' };
const source = `/// A saw.\ninstr saw(freq: hz, cutoff: 200hz..4khz = 600hz) {\n  osc(saw, freq).lpf(cutoff: cutoff)\n}\n`;

it('writes an LFO into the top of the instrument body and reads it back', () => {
  const written = setLfo(source, cutoff, { shape: 'tri', rate: 2, depth: 0.5 })!;
  expect(written).toBe(`/// A saw.\ninstr saw(freq: hz, cutoff: 200hz..4khz = 600hz) {\n  // mod cutoff: tri 2hz 50%\n  cutoff = (cutoff + lfo(2hz, shape: tri) * 950hz).clamp(200hz..4000hz)\n  osc(saw, freq).lpf(cutoff: cutoff)\n}\n`);
  expect(readLfos(written)).toEqual({ cutoff: { shape: 'tri', rate: 2, depth: 0.5 } });
});

it('replaces an LFO in place and removes it again', () => {
  const once = setLfo(source, cutoff, { shape: 'sine', rate: 1, depth: 1 })!;
  const twice = setLfo(once, cutoff, { shape: 'saw', rate: 4, depth: 0.25 })!;
  expect(twice.match(/mod cutoff/g)).toHaveLength(1);
  expect(readLfos(twice).cutoff).toEqual({ shape: 'saw', rate: 4, depth: 0.25 });
  expect(setLfo(twice, cutoff, null)).toBe(source);
});

it('keeps several parameters apart, writes plain numbers without a unit, and needs an instrument', () => {
  const res = { name: 'res', min: 0, max: 1, default: 0.3, unit: null };
  const both = setLfo(setLfo(source, cutoff, { shape: 'sine', rate: 1, depth: 1 })!, res, { shape: 'square', rate: 3, depth: 0.2 })!;
  expect(Object.keys(readLfos(both)).sort()).toEqual(['cutoff', 'res']);
  expect(both).toContain('res = (res + lfo(3hz, shape: square) * 0.1).clamp(0..1)');
  expect(setLfo('not an instrument', cutoff, { shape: 'sine', rate: 1, depth: 1 })).toBeNull();
});

it('starts the body on its own line when the brace has code after it', () => {
  const oneLine = 'instr a(freq: hz, x: 0..1 = 0.5) { osc(sine, freq) * x }';
  const written = setLfo(oneLine, { name: 'x', min: 0, max: 1, default: 0.5, unit: null }, { shape: 'sine', rate: 1, depth: 1 })!;
  expect(written.split('\n')).toHaveLength(4);
  expect(written.endsWith(' osc(sine, freq) * x }')).toBe(true);
});
