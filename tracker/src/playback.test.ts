import { expect, it } from 'vitest';
import { guessPlayback, readPlayback, writePlayback } from './playback';
import { kitSource, samplerSource, slicedSource } from './instruments';

const hit = samplerSource('kick', 'samples/kick.wav', 0.3), pad = samplerSource('pad', 'samples/pad.wav', 5);

it('reads how a sampler plays from its gate and its loop', () => {
  expect(readPlayback(hit)).toBe('one-shot'); expect(readPlayback(pad)).toBe('hold');
  expect(readPlayback(writePlayback(hit, 'loop'))).toBe('loop'); expect(readPlayback(writePlayback(hit, 'ping-pong'))).toBe('ping-pong');
  expect(readPlayback('instr x(freq: hz) { osc(sine, freq) }')).toBeNull();
});

it('changes between the four ways to play, keeping the loop points and turning the loop off again', () => {
  const looped = writePlayback(hit, 'loop');
  expect(looped).toContain('.loop(forward)'); expect(looped).toContain('gate: 0..1 = 1');
  const moved = looped.replace('loop_start: 0..1 = 0', 'loop_start: 0..1 = 0.25');
  expect(writePlayback(moved, 'ping-pong')).toContain('loop_start: 0..1 = 0.25');
  const held = writePlayback(moved, 'hold');
  expect(held).toContain('.loop(none)'); expect(held).toContain('gate: 0..1 = 1'); expect(readPlayback(held)).toBe('hold');
  expect(readPlayback(writePlayback(held, 'one-shot'))).toBe('one-shot');
  expect(writePlayback(pad, 'one-shot')).toContain('gate: 0..1 = 0');
});

it('lets a cut sample or a kit be a one-shot or held, but not looped', () => {
  const cut = slicedSource('loop', 'samples/a.wav', 4), kit = kitSource('drums', 'samples/drums', ['a', 'b']);
  expect(readPlayback(cut)).toBe('hold'); expect(readPlayback(kit)).toBe('one-shot');
  expect(readPlayback(writePlayback(cut, 'one-shot'))).toBe('one-shot'); expect(readPlayback(writePlayback(kit, 'hold'))).toBe('hold');
});

it('guesses from the name first, then the length', () => {
  expect(guessPlayback('Warm Pad C', 1, 2)).toBe('loop'); expect(guessPlayback('808_kick_long', 3, 2)).toBe('one-shot');
  expect(guessPlayback('vocal', 4, 2)).toBe('hold'); expect(guessPlayback('vocal', 0.5, 2)).toBe('one-shot');
});
