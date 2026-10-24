// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { ProjectPanel, neighbour } from './projectpanel';

afterEach(() => document.body.replaceChildren());

function setup(current = 'b') {
  const root = document.createElement('section'); document.body.append(root);
  let chosen: string | null = current, items = [{ id: 'a', name: 'Kick' }, { id: 'b', name: 'Bass', note: '×2' }, { id: 'c', name: 'Pad' }];
  const actions = { choose: vi.fn((id: string) => { chosen = id; }), edit: vi.fn(), manage: vi.fn(), leave: vi.fn() };
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

it('passes its buttons on, and follows the project when it is drawn again', () => {
  const { root, panel, actions, rows, setItems } = setup();
  for (const [label, op] of [['New instrument', 'new'], ['Duplicate the current instrument', 'duplicate'], ['Delete the current instrument', 'delete'], ['New sampler from WAV files', 'import']] as const) {
    root.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)!.click(); expect(actions.manage).toHaveBeenLastCalledWith(op);
  }
  setItems([{ id: 'b', name: 'Bass renamed' }]); panel.refresh();
  expect(rows().map(r => r.firstChild!.textContent)).toEqual(['00  Bass renamed']); expect(rows()[0].getAttribute('aria-selected')).toBe('true');
});
