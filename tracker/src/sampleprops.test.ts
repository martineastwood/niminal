import { expect, it } from 'vitest';
import { noteName, readInterpolation, readRelease, readRoot, writeInterpolation, writeRelease, writeRoot } from './sampleprops';
import { readFitMode, readFit, withFit, withFitMode } from './stretch';
import { samplerSource, slicedSource } from './instruments';

const sampler = samplerSource('pad', 'samples/pad.wav'), sliced = slicedSource('amen', 'samples/amen.wav', 4);

it('names notes as the tracker does', () => { expect(noteName(60)).toBe('C-4'); expect(noteName(61)).toBe('C#4'); expect(noteName(12)).toBe('C-0'); });

it('reads and writes the root pitch, with cents as a frequency', () => {
  expect(readRoot(sampler)).toEqual({ midi: 60, cents: 0 });
  const down = writeRoot(sampler, { midi: 57, cents: 0 });
  expect(down).toContain('.root(a3)'); expect(readRoot(down)).toEqual({ midi: 57, cents: 0 });
  const sharp = writeRoot(sampler, { midi: 61, cents: 0 }); expect(readRoot(sharp)).toEqual({ midi: 61, cents: 0 });
  const tuned = writeRoot(sampler, { midi: 60, cents: 20 });
  expect(tuned).toMatch(/\.root\(26\d\.\d+hz\)/); expect(readRoot(tuned)).toEqual({ midi: 60, cents: 20 });
  expect(readRoot(writeRoot(tuned, { midi: 50, cents: -35 }))).toEqual({ midi: 50, cents: -35 });
  expect(readRoot(writeRoot(sliced, { midi: 62, cents: 0 }))).toEqual({ midi: 62, cents: 0 });
  expect(readRoot('instr x(freq: hz) { osc(sine, freq) }')).toBeNull(); expect(writeRoot('x', { midi: 60, cents: 0 })).toBe('x');
});

it('reads and sets the release', () => {
  expect(readRelease(sampler)).toBe(10); expect(readRelease(writeRelease(sampler, 250))).toBe(250);
  expect(writeRelease(sampler, 250)).toContain('release: 0ms..2sec = 250ms'); expect(readRelease('instr x() {}')).toBeNull();
});

it('sets the interpolation on the binding the instrument plays through', () => {
  expect(readInterpolation(sampler)).toBe('cubic');
  const sinc = writeInterpolation(sampler, 'sinc');
  expect(sinc).toContain('sample("samples/pad.wav").root(c4).interp(sinc)'); expect(readInterpolation(sinc)).toBe('sinc');
  expect(writeInterpolation(sinc, 'cubic')).toBe(sampler);
  const cut = writeInterpolation(sliced, 'sinc');
  expect(cut).toContain('.slices(4).interp(sinc)'); expect(cut).not.toContain('root(c4).interp'); expect(readInterpolation(cut)).toBe('sinc');
  const linear = writeInterpolation(sinc, 'linear');
  expect(linear).toContain('.root(c4).interp(linear)'); expect(linear).not.toContain('sinc'); expect(readInterpolation(linear)).toBe('linear');
  expect(writeInterpolation(linear, 'cubic')).toBe(sampler);
  expect(readInterpolation('instr x() {}')).toBeNull();
});

it('sets how a fit follows the tempo, keeping the fit and adding the parameter to older samplers', () => {
  const fitted = withFit(sampler, 120);
  expect(readFitMode(fitted)).toBe('stretch');
  const tape = withFitMode(fitted, 'repitch');
  expect(readFitMode(tape)).toBe('repitch'); expect(readFit(tape)).toBe(120); expect(tape).toContain('from_bpm: 0..400 = 120, fit_mode: 0..1 = 1');
  expect(withFitMode(tape, 'stretch')).toContain('fit_mode: 0..1 = 0'); expect(withFitMode(tape, 'stretch').match(/fit_mode: 0\.\.1/g)).toHaveLength(1);
  expect(readFit(withFit(tape, 90))).toBe(90); expect(readFitMode(withFit(tape, 90))).toBe('repitch');
});
