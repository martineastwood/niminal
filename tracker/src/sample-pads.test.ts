import { expect, it } from 'vitest';
import { readPads, writePads } from './sample-pads';
import { slicedSource } from './instruments';
import fixture from './test-fixtures/pads.nml?raw';

it('generates the exact pad source compiled and rendered by the backend audio test', () => {
  const pads = [{ member: 1, gain: -6, tune: 12, choke: 1 }, { member: 0, gain: 0, tune: 0, choke: 1 }];
  const source = writePads(slicedSource('chops', 'samples/hits.wav', 2), pads, ['s0', 's1']);
  expect(source).toBe(fixture);
  expect(readPads(source, 2)).toEqual(pads);
  expect(writePads(source, pads, ['s0', 's1'])).toBe(source);
  expect(source).toContain('sample: sample'); // Preserve the compiler’s kit/choke metadata.
});
it('bounds malformed pad metadata and resets nonexistent mappings', () => {
  expect(readPads('// tracker-pads: [{"member":99,"gain":999,"tune":-999,"choke":99}]\n', 1)).toEqual([{ member: 0, gain: 12, tune: -24, choke: 8 }]);
  expect(readPads('// tracker-pads: broken\n', 1)).toEqual([{ member: 0, gain: 0, tune: 0, choke: 0 }]);
});
