import { expect, it } from 'vitest';
import { folderTree, searchItems, type BrowserItem } from './browser';

const items: BrowserItem[] = [
  { value: 'kick_808', name: 'Kick 808', folder: 'drums/kicks', description: 'A round kick' },
  { value: 'kick_deep', name: 'Kick deep', folder: 'drums/kicks', description: 'A deep sub kick' },
  { value: 'snare', name: 'Snare', folder: 'drums/snares', description: 'A snare' },
  { value: 'bass', name: 'Bass', folder: 'synths', description: 'A round bass' },
  { value: 'blank', name: 'Blank', folder: '' },
];

it('arranges items in folders split at the slash', () => {
  const tree = folderTree(items);
  expect([...tree.folders.keys()]).toEqual(['drums', 'synths']);
  expect([...tree.folders.get('drums')!.folders.keys()]).toEqual(['kicks', 'snares']);
  expect(tree.folders.get('drums')!.folders.get('kicks')!.items.map(i => i.value)).toEqual(['kick_808', 'kick_deep']);
  expect(tree.items.map(i => i.value)).toEqual(['blank']);
});

it('searches names, folders and descriptions for every word', () => {
  expect(searchItems(items, 'kick').map(i => i.value)).toEqual(['kick_808', 'kick_deep']);
  expect(searchItems(items, 'deep sub').map(i => i.value)).toEqual(['kick_deep']);
  expect(searchItems(items, 'snares').map(i => i.value)).toEqual(['snare']);
  expect(searchItems(items, 'round').map(i => i.value)).toEqual(['kick_808', 'bass']);
  expect(searchItems(items, '  ').length).toBe(items.length);
  expect(searchItems(items, 'zzz')).toEqual([]);
});
