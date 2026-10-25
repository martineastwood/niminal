import { expect, it } from 'vitest';
import { samplerSource, slicedSource } from './instruments';
import { readLoop, writeLoop } from './loops';

it('adds sustain looping and forwards its bounds, keeping one sample and no slices', () => {
  const original = samplerSource('pad', 'pad.wav');
  const loop = { mode: 'forward' as const, start: 0.2, end: 0.8 };
  const source = writeLoop(original, loop);
  expect(readLoop(source)).toEqual(loop);
  expect(source).toContain('.root(c4).loop(forward)');
  expect(source).toContain('loop_start: loop_start, loop_end: loop_end');
  expect(source).toContain('gate: 0..1 = 1');
  expect(source).not.toContain('.slices(');
  expect(writeLoop(source, loop)).toBe(source);
  const pingpong = writeLoop(source, { ...loop, mode: 'bidirectional' });
  expect(readLoop(pingpong).mode).toBe('bidirectional');
  expect(readLoop(writeLoop(pingpong, { ...loop, mode: 'none' })).mode).toBe('none');
});

it('rejects invalid bounds and leaves other sample methods intact', () => {
  const source = samplerSource('pad', 'pad.wav').replace('.root(c4)', '.root(c4).interp(sinc).loop(forward)');
  expect(writeLoop(source, { mode: 'forward', start: 0.8, end: 0.2 })).toBe(source);
  expect(writeLoop(source, { mode: 'forward', start: NaN, end: 1 })).toBe(source);
  expect(writeLoop(source, { mode: 'forward', start: 0, end: 0.5 })).toContain('.root(c4).interp(sinc).loop(forward)');
  expect(readLoop(slicedSource('chops', 'break.wav', 4)).mode).toBe('none');
});

it('round trips crossfade milliseconds and rejects invalid durations', () => {
  const original = samplerSource('pad', 'pad.wav');
  const loop = { mode: 'forward' as const, start: 0.2, end: 0.8, crossfade: 12.5 };
  const source = writeLoop(original, loop);
  expect(readLoop(source)).toEqual(loop);
  expect(source).toContain('loop_crossfade: sec = 12.5ms');
  expect(source).toContain('loop_crossfade: loop_crossfade');
  expect(writeLoop(source, loop)).toBe(source);
  expect(writeLoop(source, { ...loop, crossfade: -1 })).toBe(source);
  expect(writeLoop(source, { ...loop, crossfade: NaN })).toBe(source);
});
