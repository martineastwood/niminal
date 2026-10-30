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
const samples: BrowserItem[] = [{ value: 'sample:samples/kick.wav', name: 'kick', folder: '' }, { value: 'sample:samples/kit/hat.wav', name: 'hat', folder: 'kit' }];

function setup() {
  const root = document.createElement('section'); document.body.append(root);
  const actions = { audition: vi.fn(), pick: vi.fn(), menu: vi.fn(), group: vi.fn(), manage: vi.fn(), leave: vi.fn() }, focusGrid = vi.fn();
  const browser = new SoundBrowser(root, kind => kind === 'instr' ? instruments : kind === 'sample' ? samples : effects, actions, focusGrid);
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
beforeEach(() => { vi.useFakeTimers(); localStorage.clear(); });
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

it('keeps samples silent while moving over them: click or Space plays', () => {
  const { root, actions, key, browser } = setup();
  browser.show('sample', false);
  key('ArrowDown'); vi.advanceTimersByTime(200); expect(actions.audition).not.toHaveBeenCalled();
  root.querySelector<HTMLElement>('.sb-item')!.click(); vi.advanceTimersByTime(1); expect(actions.audition).toHaveBeenCalledWith('sample', samples[0]);
});

it('groups samples with Cmd-click and Shift-click, and offers the group on Enter, a right-click, or a folder right-click', () => {
  const { root, actions, key, browser } = setup();
  browser.show('sample', false);
  [...root.querySelectorAll<HTMLButtonElement>('.sb-folder')][0].click();
  const rows = () => [...root.querySelectorAll<HTMLElement>('.sb-item')];
  rows()[0].click(); vi.advanceTimersByTime(1);
  rows()[1].dispatchEvent(new MouseEvent('click', { shiftKey: true, bubbles: true }));
  expect(rows().map(r => r.getAttribute('aria-selected'))).toEqual(['true', 'true']);
  actions.audition.mockClear(); key('Enter');
  expect(actions.group).toHaveBeenLastCalledWith(samples, expect.anything()); expect(actions.pick).not.toHaveBeenCalled();
  rows()[0].dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true })); expect(actions.group).toHaveBeenCalledTimes(2);
  rows()[0].dispatchEvent(new MouseEvent('click', { metaKey: true, bubbles: true }));
  expect(rows().map(r => r.getAttribute('aria-selected'))).toEqual(['false', 'true']); expect(actions.audition).not.toHaveBeenCalled();
  [...root.querySelectorAll<HTMLButtonElement>('.sb-folder')][0].dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  expect(actions.group).toHaveBeenLastCalledWith([samples[1]], expect.anything());
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
  let list: BrowserItem[] = [{ value: 'a', name: '00  Lead', folder: '', note: '×2' }, { value: 'b', name: '01  Pad', folder: '' }];
  const actions = { audition: vi.fn(), pick: vi.fn(), menu: vi.fn(), group: vi.fn(), manage: vi.fn(), leave: vi.fn() };
  const browser = new SoundBrowser(root, () => list, actions, vi.fn());
  const input = root.querySelector('input')!;
  for (let i = 0; i < 2; i++) input.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true })); // Lead, Pad
  expect(root.querySelector('.sb-item[aria-selected=true]')!.textContent).toBe('01  Pad');
  expect(root.querySelector('.sb-item .browser-path')!.textContent).toBe('×2');
  list = [{ value: 'b', name: '00  Pad', folder: '' }];
  browser.refresh();
  expect(root.querySelector('.sb-item[aria-selected=true]')!.textContent).toBe('00  Pad');
  list = [];
  browser.refresh();
  expect(root.querySelector('.sb-item[aria-selected=true]')).toBeNull();
});

it('has footer buttons that belong to the tab: none for instruments, folder and import for samples', () => {
  const { root, actions, browser } = setup();
  const tools = root.querySelector<HTMLElement>('.sb-tools')!, labels = () => [...tools.querySelectorAll('button')].map(b => b.textContent);
  expect(tools.hidden).toBe(true);
  browser.show('sample', false); expect(tools.hidden).toBe(false); expect(labels()).toEqual(['Choose folder…', 'Import files…', 'Import as kit…']);
  tools.querySelectorAll('button')[0].click(); expect(actions.manage).toHaveBeenLastCalledWith('folder', 'sample');
});

it('lists the samples of the library folder in their folders, and tells an empty one how to add them', () => {
  const { root, browser } = setup();
  browser.show('sample', false);
  expect([...root.querySelectorAll('.sb-folder')].map(f => f.textContent)).toEqual(['▸ kit']); // no folder of the project's own: the loose samples are at the top
  expect([...root.querySelectorAll('.sb-name')].map(n => n.textContent)).toEqual(['kick']);
  const empty = document.createElement('section'); document.body.append(empty);
  new SoundBrowser(empty, () => [], { audition: vi.fn(), pick: vi.fn(), menu: vi.fn(), group: vi.fn(), manage: vi.fn(), leave: vi.fn() }, vi.fn()).show('sample', false);
  expect(empty.querySelector('.sb-none')!.textContent).toContain('Choose a folder');
});

it('stars a row to list it under Favourites, and remembers it', () => {
  const { root, browser } = setup();
  openAll(root, browser);
  const star = root.querySelector<HTMLButtonElement>('.sb-item .sb-star')!;
  star.click();
  expect([...root.querySelectorAll('.sb-folder')][0].textContent).toBe('▾ Favourites');
  expect(root.querySelector('.sb-item')!.firstChild!.textContent).toBe('Kick'); // the first row of the Favourites folder
  const again = document.createElement('section'); document.body.append(again);
  new SoundBrowser(again, () => instruments, { audition: vi.fn(), pick: vi.fn(), menu: vi.fn(), group: vi.fn(), manage: vi.fn(), leave: vi.fn() }, vi.fn());
  expect(again.querySelector('.sb-folder')!.textContent).toBe('▾ Favourites');
  root.querySelector<HTMLButtonElement>('.sb-item .sb-star')!.click();
  expect([...root.querySelectorAll('.sb-folder')].map(f => f.textContent)).not.toContain('▾ Favourites');
});

it('opens a menu for a row from its button or a right-click, selecting it first', () => {
  const { root, actions, browser } = setup();
  openAll(root, browser);
  const more = root.querySelector<HTMLButtonElement>('.sb-item .sb-more')!;
  more.click(); expect(actions.menu).toHaveBeenLastCalledWith('instr', instruments[0], more);
  vi.advanceTimersByTime(10); expect(actions.audition).toHaveBeenCalledWith('instr', instruments[0]);
  const row = root.querySelectorAll<HTMLElement>('.sb-item')[1];
  row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true })); expect(actions.menu).toHaveBeenLastCalledWith('instr', instruments[1], row);
});
