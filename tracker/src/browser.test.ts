// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { folderTree, openBrowser, searchItems, type BrowserItem } from './browser';

const items: BrowserItem[] = [
  { value: 'kick_808', name: 'Kick 808', folder: 'drums/kicks', description: 'A round kick' },
  { value: 'kick_deep', name: 'Kick deep', folder: 'drums/kicks', description: 'A deep sub kick' },
  { value: 'snare', name: 'Snare', folder: 'drums/snares', description: 'A snare' },
  { value: 'bass', name: 'Bass', folder: 'synths', description: 'A round bass' },
  { value: 'blank', name: 'Blank', folder: '' },
];

afterEach(() => { document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' })); document.body.replaceChildren(); });

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

it('shows folders with their counts, picks an item and closes', () => {
  const anchor = document.createElement('button'); document.body.append(anchor);
  const pick = vi.fn(); openBrowser(anchor, items, pick);
  const summaries = [...document.querySelectorAll('summary')].map(s => s.textContent);
  expect(summaries).toEqual(['drums (3)', 'kicks (2)', 'snares (1)', 'synths (1)']);
  [...document.querySelectorAll<HTMLButtonElement>('.browser-item')].find(b => b.textContent === 'Kick deep')!.click();
  expect(pick).toHaveBeenCalledWith(items[1]); expect(document.querySelector('.browser')).toBeNull();
});

it('lists matches from every folder while searching, and Enter picks the first', () => {
  const anchor = document.createElement('button'); document.body.append(anchor);
  const pick = vi.fn(); openBrowser(anchor, items, pick);
  const search = document.querySelector<HTMLInputElement>('input[type=search]')!;
  search.value = 'kick'; search.dispatchEvent(new Event('input'));
  expect([...document.querySelectorAll('.browser-item')].map(b => b.textContent)).toEqual(['Kick 808drums/kicks', 'Kick deepdrums/kicks']);
  expect(document.querySelector('summary')).toBeNull();
  search.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' })); expect(pick).toHaveBeenCalledWith(items[0]);
  openBrowser(anchor, items, pick);
  const again = document.querySelector<HTMLInputElement>('input[type=search]')!; again.value = 'zzz'; again.dispatchEvent(new Event('input'));
  expect(document.querySelector('.browser-none')?.textContent).toBe('Nothing matches');
});

it('shows what the hovered item is, and closes on Escape or a click elsewhere', () => {
  const anchor = document.createElement('button'); document.body.append(anchor);
  openBrowser(anchor, items, vi.fn());
  const kick = [...document.querySelectorAll<HTMLButtonElement>('.browser-item')].find(b => b.textContent === 'Kick 808')!;
  kick.dispatchEvent(new Event('mouseenter')); expect(document.querySelector('.browser-hint')!.textContent).toBe('A round kick');
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' })); expect(document.querySelector('.browser')).toBeNull();
  openBrowser(anchor, items, vi.fn());
  document.body.dispatchEvent(new Event('pointerdown', { bubbles: true })); expect(document.querySelector('.browser')).toBeNull();
});
