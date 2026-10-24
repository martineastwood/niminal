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
beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); document.body.replaceChildren(); });

it('plays the item the arrow keys move to, after a short pause, and adds it on Enter', () => {
  const { actions, key } = setup();
  key('ArrowDown'); key('ArrowDown');
  expect(actions.audition).not.toHaveBeenCalled(); // still scrolling
  vi.advanceTimersByTime(200);
  expect(actions.audition).toHaveBeenCalledOnce(); expect(actions.audition).toHaveBeenCalledWith('instr', instruments[1]);
  key('Enter'); expect(actions.pick).toHaveBeenCalledWith('instr', instruments[1]);
});

it('walks the folders in the order they are drawn, and stops at the ends', () => {
  const { actions, key } = setup();
  const heard = () => { vi.advanceTimersByTime(200); return actions.audition.mock.calls.at(-1)![1].value; };
  key('ArrowDown'); expect(heard()).toBe('kick');
  key('ArrowDown'); expect(heard()).toBe('snare');
  key('ArrowDown'); expect(heard()).toBe('bass');
  key('ArrowDown'); expect(heard()).toBe('bass');
  key('ArrowUp'); expect(heard()).toBe('snare');
});

it('searches across folders, and switches between instruments and effects', () => {
  const { root, actions, key, browser } = setup();
  const search = root.querySelector('input')!; search.value = 'round'; search.dispatchEvent(new Event('input'));
  expect([...root.querySelectorAll('.sb-item')].map(b => b.firstChild!.textContent)).toEqual(['Bass']);
  key('ArrowDown'); vi.advanceTimersByTime(200); expect(actions.audition).toHaveBeenCalledWith('instr', instruments[2]);
  browser.show('fx', false);
  expect([...root.querySelectorAll('.sb-item')].map(b => b.firstChild!.textContent)).toEqual(['Echo']);
  expect(root.querySelector('[aria-selected=true]')!.textContent).toBe('Effects');
});

it('collapses a folder so the arrow keys skip what it holds', () => {
  const { root, actions, key } = setup();
  [...root.querySelectorAll<HTMLButtonElement>('.sb-folder')].find(b => b.textContent!.includes('drums'))!.click();
  key('ArrowDown'); vi.advanceTimersByTime(200);
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
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true })); input.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
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
  key('ArrowDown'); button('Edit the code of the selected sound of this project').click();
  expect(actions.manage).toHaveBeenCalledWith('edit', 'instr', instruments[0]);
  expect(button('New sampler from WAV files').hidden).toBe(false);
  browser.show('fx', false); button('New blank sound').click();
  expect(actions.manage).toHaveBeenLastCalledWith('new', 'fx', null); expect(button('New sampler from WAV files').hidden).toBe(true);
});
