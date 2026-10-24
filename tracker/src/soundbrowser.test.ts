// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { SoundBrowser } from './soundbrowser';
import type { BrowserItem } from './browser';

const instruments: BrowserItem[] = [
  { value: 'kick', name: 'Kick', folder: 'drums/kicks', description: 'A kick' },
  { value: 'snare', name: 'Snare', folder: 'drums/snares', description: 'A snare' },
  { value: 'bass', name: 'Bass', folder: 'synths', description: 'A round bass' },
];
const effects: BrowserItem[] = [{ value: 'echo', name: 'Echo', folder: 'time/delay', description: 'An echo' }];

function setup() {
  const root = document.createElement('section'); document.body.append(root);
  const actions = { audition: vi.fn(), pick: vi.fn(), manage: vi.fn(), leave: vi.fn() }, focusGrid = vi.fn();
  const browser = new SoundBrowser(root, kind => kind === 'instr' ? instruments : effects, actions, focusGrid);
  const key = (name: string, target: Element = root.querySelector('input')!) => target.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true }));
  return { root, actions, focusGrid, browser, key };
}
/** Folders start closed; open every one of them, with the cursor back at the top. */
function openAll(root: HTMLElement, browser: SoundBrowser) {
  for (;;) {
    const closed = [...root.querySelectorAll<HTMLButtonElement>('.sb-folder')].find(folder => folder.textContent!.startsWith('▸'));
    if (!closed) break;
    closed.click();
  }
  browser.show('instr', false); // clicking put the cursor on a folder; this puts it back at the top and keeps them open
}
beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); document.body.replaceChildren(); });

it('starts with its folders closed, and opens and closes one with Enter or the right and left arrows', () => {
  const { root, actions, key } = setup();
  const folders = () => [...root.querySelectorAll('.sb-folder')].map(f => f.textContent);
  expect(folders()).toEqual(['▸ drums', '▸ synths']); expect(root.querySelector('.sb-item')).toBeNull();
  key('ArrowDown'); key('Enter'); expect(folders()).toEqual(['▾ drums', '▸ kicks', '▸ snares', '▸ synths']);
  key('ArrowLeft'); expect(folders()).toEqual(['▸ drums', '▸ synths']);
  key('ArrowRight'); key('ArrowDown'); key('ArrowRight'); expect(root.querySelector('.sb-item')!.textContent).toBe('Kick');
  vi.advanceTimersByTime(200); expect(actions.audition).not.toHaveBeenCalled(); // landing on a folder plays nothing
});

it('plays the item the arrow keys move to, after a short pause, and adds it on Enter', () => {
  const { root, actions, key, browser } = setup();
  openAll(root, browser);
  for (let i = 0; i < 5; i++) key('ArrowDown'); // drums, kicks, Kick, snares, Snare
  expect(actions.audition).not.toHaveBeenCalled(); // still scrolling
  vi.advanceTimersByTime(200);
  expect(actions.audition).toHaveBeenCalledOnce(); expect(actions.audition).toHaveBeenCalledWith('instr', instruments[1]);
  key('Enter'); expect(actions.pick).toHaveBeenCalledWith('instr', instruments[1], false);
  root.querySelector('input')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', shiftKey: true, bubbles: true }));
  expect(actions.pick).toHaveBeenLastCalledWith('instr', instruments[1], true); // Shift+Enter loads it into the current slot
});

it('walks the folders and items in the order they are drawn, and stops at the ends', () => {
  const { root, actions, key, browser } = setup();
  openAll(root, browser);
  const heard = () => { vi.advanceTimersByTime(200); return actions.audition.mock.calls.at(-1)![1].value; };
  for (let i = 0; i < 3; i++) key('ArrowDown'); expect(heard()).toBe('kick');
  for (let i = 0; i < 2; i++) key('ArrowDown'); expect(heard()).toBe('snare');
  for (let i = 0; i < 2; i++) key('ArrowDown'); expect(heard()).toBe('bass');
  key('ArrowDown'); expect(heard()).toBe('bass');
  key('ArrowUp'); key('ArrowUp'); expect(heard()).toBe('snare');
});

it('searches across folders, and switches between instruments and effects', () => {
  const { root, actions, key, browser } = setup();
  const search = root.querySelector('input')!; search.value = 'round'; search.dispatchEvent(new Event('input'));
  expect([...root.querySelectorAll('.sb-item')].map(b => b.firstChild!.textContent)).toEqual(['Bass']);
  key('ArrowDown'); vi.advanceTimersByTime(200); expect(actions.audition).toHaveBeenCalledWith('instr', instruments[2]);
  browser.show('fx', false);
  expect([...root.querySelectorAll('.sb-folder')].map(f => f.textContent)).toEqual(['▸ time']);
  expect(root.querySelector('[aria-selected=true]')!.textContent).toBe('Effects');
});

it('collapses a folder so the arrow keys skip what it holds', () => {
  const { root, actions, key, browser } = setup();
  openAll(root, browser);
  [...root.querySelectorAll<HTMLButtonElement>('.sb-folder')].find(b => b.textContent!.includes('drums'))!.click();
  for (let i = 0; i < 3; i++) key('ArrowDown'); // drums, synths, Bass
  vi.advanceTimersByTime(200);
  expect(actions.audition).toHaveBeenCalledWith('instr', instruments[2]);
});

it('stops previewing and gives the grid focus on Escape', () => {
  const { actions, focusGrid, key } = setup();
  key('ArrowDown'); key('Escape');
  vi.advanceTimersByTime(200);
  expect(actions.leave).toHaveBeenCalled(); expect(focusGrid).toHaveBeenCalled(); expect(actions.audition).not.toHaveBeenCalled();
});

it('lists what the project gives it again when asked, keeping the selection, and shows a note at the end of a row', () => {
  const root = document.createElement('section'); document.body.append(root);
  let list: BrowserItem[] = [{ value: 'project:a', name: '00  Lead', folder: 'Project', note: '×2' }, { value: 'project:b', name: '01  Pad', folder: 'Project' }];
  const actions = { audition: vi.fn(), pick: vi.fn(), manage: vi.fn(), leave: vi.fn() };
  const browser = new SoundBrowser(root, () => list, actions, vi.fn());
  const input = root.querySelector('input')!;
  for (let i = 0; i < 3; i++) input.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true })); // Project (open already), Lead, Pad
  expect(root.querySelector('.sb-item[aria-selected=true]')!.textContent).toBe('01  Pad');
  expect(root.querySelector('.sb-item .browser-path')!.textContent).toBe('×2');
  list = [{ value: 'project:b', name: '00  Pad', folder: 'Project' }];
  browser.refresh();
  expect(root.querySelector('.sb-item[aria-selected=true]')!.textContent).toBe('00  Pad');
  list = [];
  browser.refresh();
  expect(root.querySelector('.sb-item[aria-selected=true]')).toBeNull();
});

it('passes the buttons under the list on with the tab and the selected item, and offers samplers only for instruments', () => {
  const { root, actions, key, browser } = setup();
  const button = (title: string) => root.querySelector<HTMLButtonElement>(`.sb-tools [aria-label="${title}"]`)!;
  openAll(root, browser); for (let i = 0; i < 3; i++) key('ArrowDown');
  button('Edit the code of the selected sound of this project').click();
  expect(actions.manage).toHaveBeenCalledWith('edit', 'instr', instruments[0]);
  expect(button('New sampler from WAV files').hidden).toBe(false);
  browser.show('fx', false); button('New blank sound').click();
  expect(actions.manage).toHaveBeenLastCalledWith('new', 'fx', null); expect(button('New sampler from WAV files').hidden).toBe(true);
});
