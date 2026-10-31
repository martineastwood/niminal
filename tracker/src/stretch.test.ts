import { expect, it } from 'vitest';
import { readFit, sourceTempo, withFit, estimateBeats } from './stretch';
import { samplerSource } from './instruments';

it('sets and reads the tempo a sample is fitted from', () => {
  const fresh = samplerSource('hit', 'samples/hit.wav');
  expect(readFit(fresh)).toBe(0);
  expect(readFit(withFit(fresh, 136.5))).toBe(136.5); expect(withFit(fresh, 136.5)).toContain('from_bpm: 0..400 = 136.5');
  expect(readFit(withFit(withFit(fresh, 136.5), 0))).toBe(0);
  expect(readFit('instr x(freq: hz) { 0 }')).toBe(0);
});

it('works out the tempo from the beats a sample lasts, within what a tempo can be', () => {
  expect(sourceTempo(4, 2)).toBe(120); expect(sourceTempo(8, 3.5)).toBe(137.14); expect(sourceTempo(1000, 1)).toBeNull(); expect(sourceTempo(1, 0)).toBeNull(); expect(sourceTempo(0, 1)).toBeNull();
});

it('estimates a beat count from regular hits, subdivisions and missing beats', () => {
  expect(estimateBeats([0, 0.25, 0.5, 0.75], 2)).toEqual({ beats: 4, bpm: 120 });
  expect(estimateBeats(Array.from({ length: 16 }, (_, i) => i / 16), 2)).toEqual({ beats: 4, bpm: 120 });
  expect(estimateBeats([0, 0.125, 0.375, 0.5, 0.625, 0.875], 4)).toEqual({ beats: 8, bpm: 120 });
  expect(estimateBeats([0, 0.25, 0.5, 0.75], 60 * 4 / 137)).toEqual({ beats: 4, bpm: 137 });
});

it('rejects insufficient evidence and invalid duration instead of inventing a tempo', () => {
  for (const hits of [[], [0], [0, 0.5], [0, 0.2, 0.6], [NaN, 0, 0, Infinity]]) expect(estimateBeats(hits, 2)).toBeNull();
  expect(estimateBeats([0, 0.25, 0.5, 0.75], 0)).toBeNull();
  expect(estimateBeats([0, 0.25, 0.5, 0.75], NaN)).toBeNull();
  expect(estimateBeats([0, 0.117, 0.333, 0.721, 0.936], 2)).toBeNull();
});


it('tolerates small onset jitter, duplicate detections and unordered hits', () => {
  expect(estimateBeats([0.7505, 0, 0.2508, 0.4993, 0.2508, NaN, -1, 1], 2)).toEqual({ beats: 4, bpm: 120 });
});
