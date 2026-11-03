import { expect, it } from 'vitest';
import { assetUsers, missingAssets, type SampleAsset } from './sample-assets';
import type { Definition } from './project';
const definitions: Definition[] = [
  { id: 'a', name: 'Kick', kind: 'instr', source: 'x = sample("samples/kick.wav")\n// Original sample: samples/old.wav' },
  { id: 'b', name: 'Kit', kind: 'instr', source: 'x = kit("samples/kit")' },
];
it('finds direct and folder-kit users, excluding provenance comments', () => {
  expect(assetUsers(definitions, 'samples/kick.wav').map(d => d.id)).toEqual(['a']);
  expect(assetUsers(definitions, 'samples/kit/hat.wav').map(d => d.id)).toEqual(['b']);
  expect(assetUsers(definitions, 'samples/old.wav')).toEqual([]);
});
it('shows missing active sample references once', () => {
  expect(missingAssets([...definitions, definitions[0]], []).map(a => a.file)).toEqual(['samples/kick.wav']);
  expect(missingAssets(definitions, [{ file: 'samples/kick.wav' } as SampleAsset])).toEqual([]);
});
