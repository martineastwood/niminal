// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { ProjectPanel, neighbour, type ProjectInstrument } from './projectpanel';

afterEach(() => document.body.replaceChildren());

function setup(current = 'b') {
  const root = document.createElement('section'); document.body.append(root);
  let chosen: string | null = current, items: ProjectInstrument[] = [{ id: 'a', name: 'Kick' }, { id: 'b', name: 'Bass', note: '×2' }, { id: 'c', name: 'Pad' }];
  const actions = { choose: vi.fn((id: string) => { chosen = id; }), edit: vi.fn(), add: vi.fn(), menu: vi.fn(), remove: vi.fn(), drop: vi.fn(), leave: vi.fn() };
  const panel = new ProjectPanel(root, () => items, () => chosen, actions);
  const list = root.querySelector<HTMLElement>('.pp-list')!;
  const key = (name: string, options: KeyboardEventInit = {}) => list.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true, ...options }));
  return { root, panel, actions, key, rows: () => [...root.querySelectorAll<HTMLElement>('.pp-item')], setItems: (next: typeof items) => { items = next; } };
}

it('moves to the neighbouring instrument and stops at the ends', () => {
  expect(neighbour(['a', 'b', 'c'], 'b', 1)).toBe('c'); expect(neighbour(['a', 'b', 'c'], 'b', -1)).toBe('a');
  expect(neighbour(['a', 'b', 'c'], 'c', 1)).toBeNull(); expect(neighbour(['a', 'b', 'c'], 'a', -1)).toBeNull();
  expect(neighbour([], 'a', 1)).toBeNull(); expect(neighbour(['a', 'b'], null, 1)).toBe('a'); // nothing chosen yet: the first is where it goes
});

it('lists the instruments with their numbers and a count, and marks the current one', () => {
  const { rows } = setup();
  expect(rows().map(r => r.firstChild!.textContent)).toEqual(['00  Kick', '01  Bass', '02  Pad']);
  expect(rows()[1].querySelector('small')!.textContent).toBe('×2');
  expect(rows().map(r => r.getAttribute('aria-selected'))).toEqual(['false', 'true', 'false']);
});

it('chooses with a click or the arrow keys, opens the code with Enter or a double-click, and leaves on Escape', () => {
  const { rows, key, actions } = setup();
  rows()[0].click(); expect(actions.choose).toHaveBeenLastCalledWith('a');
  key('ArrowDown'); expect(actions.choose).toHaveBeenLastCalledWith('b');
  key('ArrowDown'); expect(actions.choose).toHaveBeenLastCalledWith('c');
  key('ArrowDown'); expect(actions.choose).toHaveBeenCalledTimes(3); // already the last
  key('Enter'); expect(actions.edit).toHaveBeenLastCalledWith('c');
  rows()[1].dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); expect(actions.edit).toHaveBeenLastCalledWith('b');
  key('ArrowUp', { altKey: true }); expect(actions.choose).toHaveBeenCalledTimes(3); // Option+arrows belong to the app, not the list
  key('Escape'); expect(actions.leave).toHaveBeenCalled();
});

it('tags a sampler with how it plays', () => {
  const { panel, rows, setItems } = setup();
  setItems([{ id: 'a', name: 'Kick', tag: { text: 'one-shot', title: 'Plays to its end' } }, { id: 'b', name: 'Bass' }]); panel.refresh();
  expect(rows()[0].querySelector('.pp-tag')!.textContent).toBe('one-shot'); expect(rows()[0].querySelector('.pp-tag')!.getAttribute('title')).toBe('Plays to its end');
  expect(rows()[1].querySelector('.pp-tag')).toBeNull();
});

it('follows the project when it is drawn again', () => {
  const { panel, rows, setItems } = setup();
  setItems([{ id: 'b', name: 'Bass renamed' }]); panel.refresh();
  expect(rows().map(r => r.firstChild!.textContent)).toEqual(['00  Bass renamed']); expect(rows()[0].getAttribute('aria-selected')).toBe('true');
});

it('opens the add menu from its one labelled button', () => {
  const { root, actions } = setup();
  const add = [...root.querySelectorAll('button')].find(b => b.textContent === '+ Add')!;
  add.click(); expect(actions.add).toHaveBeenCalledWith(add);
});

it('opens a menu for the row from its button or a right-click, choosing that row first', () => {
  const { root, actions, rows } = setup();
  const more = root.querySelector<HTMLButtonElement>('[aria-label="More actions for Pad"]')!;
  more.click(); expect(actions.choose).toHaveBeenLastCalledWith('c'); expect(actions.menu).toHaveBeenLastCalledWith('c', more);
  rows()[0].dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  expect(actions.choose).toHaveBeenLastCalledWith('a'); expect(actions.menu).toHaveBeenLastCalledWith('a', rows()[0]);
});

it('deletes the current instrument with Delete or Backspace', () => {
  const { key, actions } = setup();
  key('Delete'); expect(actions.remove).toHaveBeenLastCalledWith('b');
  key('Backspace'); expect(actions.remove).toHaveBeenCalledTimes(2);
});
