import { expect, it } from 'vitest';
import { addCut, cutNear, minGap, moveCut, pieceAt, pieceKey, readCuts, removeCut, writeCuts } from './slices';
import { slicedSource } from './instruments';

it('reads equal and listed cuts from the code, and nothing from code that cuts none', () => {
  expect(readCuts('chops = amen.slices(4)')!.points).toEqual([0, 0.25, 0.5, 0.75]);
  expect(readCuts('chops = amen.slices([0, 0.18, 0.43])')!.points).toEqual([0, 0.18, 0.43]);
  expect(readCuts('kick = sample("a.wav")')).toBeNull(); expect(readCuts('x = a.slices([])')).toBeNull(); expect(readCuts('x = a.slices([0, y])')).toBeNull(); expect(readCuts('x = a.slices(0)')).toBeNull();
});

it('writes cuts back into the code the tracker writes, keeping the rest and the piece count', () => {
  const source = slicedSource('amen', 'samples/amen.wav', 4);
  const written = writeCuts(source, [0, 0.1, 0.123456, 0.9]);
  expect(written).toContain('amen_sample.slices([0, 0.1, 0.12346, 0.9])'); expect(written).toContain('in 4 pieces');
  expect(writeCuts(source, [0, 0.5, 0.7])).toContain('in 3 pieces'); expect(readCuts(writeCuts(source, [0, 0.5, 0.7]))!.points).toEqual([0, 0.5, 0.7]);
  expect(writeCuts(source, [0, 0.5]).replace(/slices\(\[0, 0.5\]\)/, '')).toBe(source.replace(/slices\(4\)/, '').replace('in 4 pieces', 'in 2 pieces'));
  expect(writeCuts('no cuts here', [0, 0.5])).toBe('no cuts here');
});

it('moves, adds and removes cuts without crossing a neighbour or touching the first', () => {
  const points = [0, 0.25, 0.5, 0.75];
  expect(moveCut(points, 1, 0.3)).toEqual([0, 0.3, 0.5, 0.75]);
  expect(moveCut(points, 1, 0.9)[1]).toBeCloseTo(0.5 - minGap); expect(moveCut(points, 2, 0.1)[2]).toBeCloseTo(0.25 + minGap); expect(moveCut(points, 3, 2)[3]).toBeCloseTo(1 - minGap);
  expect(moveCut(points, 0, 0.1)).toBe(points); expect(moveCut(points, 9, 0.1)).toBe(points);
  expect(addCut(points, 0.6)).toEqual([0, 0.25, 0.5, 0.6, 0.75]); expect(addCut(points, 0.5001)).toBe(points); expect(addCut(points, 0.0005)).toBe(points); expect(addCut(points, 1)).toBe(points);
  expect(removeCut(points, 2)).toEqual([0, 0.25, 0.75]); expect(removeCut(points, 0)).toBe(points); expect(removeCut([0, 0.5], 1)).toEqual([0, 0.5]);
});

it('finds the piece at a place, the cut near a click, and the key a piece sits on', () => {
  const points = [0, 0.25, 0.5];
  expect([0, 0.1, 0.25, 0.4, 0.99].map(share => pieceAt(points, share))).toEqual([0, 0, 1, 1, 2]);
  expect(cutNear(points, 0.255, 0.01)).toBe(1); expect(cutNear(points, 0.3, 0.01)).toBe(-1); expect(cutNear(points, 0.003, 0.01)).toBe(-1);
  expect([0, 1, 12].map(pieceKey)).toEqual(['C-4', 'C#4', 'C-5']);
});

it('writes a sliced sampler as one that plays its whole sample, keeping its settings', async () => {
  const { slicedSource, samplerSource } = await import('./instruments');
  const { unslice } = await import('./slices');
  const whole = unslice(slicedSource('amen', 'samples/amen.wav', 8))!;
  expect(whole).not.toContain('slices'); expect(whole).not.toContain('sample: num'); expect(whole).not.toContain('sample: sample');
  expect(whole).toContain('amen_sample(freq: freq, begin: begin, rate: rate, stretch: stretch, stretch_mode: stretch_mode)');
  expect(whole).toContain('/// Plays samples/amen.wav. Change .root(...)');
  expect(whole.replace(/gate: 0\.\.1 = 1/, 'gate: 0..1 = 0').replace(/\/\/\/ Plays.*\n/, '')).toContain('amen_sample = sample("samples/amen.wav").root(c4)');
  expect(whole.match(/^instr amen\(/m)).not.toBeNull();
  // the settings of the cut binding move to the sample's
  const tuned = slicedSource('amen', 'samples/amen.wav', 8).replace('.slices(8)', '.slices(8).interp(sinc)');
  expect(unslice(tuned)).toContain('amen_sample = sample("samples/amen.wav").root(c4).interp(sinc)');
  expect(unslice(samplerSource('kick', 'samples/kick.wav'))).toBeNull();
});
