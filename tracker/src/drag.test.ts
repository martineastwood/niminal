// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { draggable, dropTarget } from './drag';

afterEach(async () => { document.body.replaceChildren(); vi.restoreAllMocks(); await new Promise(done => setTimeout(done)); }); // the click swallowed after a drag ends with the tick

const pointer = (type: string, target: EventTarget, x: number, y: number) => target.dispatchEvent(new MouseEvent(type, { clientX: x, clientY: y, button: 0, bubbles: true }));

function setup(kind: 'instr' | 'fx' = 'instr') {
  const row = document.createElement('div'), slot = document.createElement('div'), list = document.createElement('div'); list.append(slot); document.body.append(row, list);
  const onSlot = vi.fn(), onList = vi.fn();
  draggable(row, () => ({ kind, value: 'kick', name: 'Kick' }));
  dropTarget(slot, item => item.kind === 'instr', onSlot); dropTarget(list, item => item.kind === 'instr', onList);
  let under: Element | null = slot;
  document.elementFromPoint = () => under;
  return { row, slot, list, onSlot, onList, over: (next: Element | null) => { under = next; } };
}

it('drops what was dragged on the innermost target under the pointer, and shows a label while it moves', () => {
  const { row, slot, onSlot, onList } = setup();
  pointer('pointerdown', row, 0, 0); pointer('pointermove', document, 20, 0);
  expect(document.querySelector('.drag-ghost')!.textContent).toBe('Kick'); expect(slot.classList.contains('drop-over')).toBe(true);
  pointer('pointerup', document, 20, 0);
  expect(onSlot).toHaveBeenCalledWith({ kind: 'instr', value: 'kick', name: 'Kick' }, slot); expect(onList).not.toHaveBeenCalled();
  expect(document.querySelector('.drag-ghost')).toBeNull(); expect(slot.classList.contains('drop-over')).toBe(false);
});

it('falls back to the outer target over empty space, and drops nothing outside any target', () => {
  const { row, list, onSlot, onList, over } = setup();
  over(list); pointer('pointerdown', row, 0, 0); pointer('pointermove', document, 20, 0); pointer('pointerup', document, 20, 0);
  expect(onList).toHaveBeenCalledOnce(); expect(onSlot).not.toHaveBeenCalled();
  over(null); pointer('pointerdown', row, 0, 0); pointer('pointermove', document, 20, 0); pointer('pointerup', document, 20, 0);
  expect(onList).toHaveBeenCalledOnce();
});

it('is a click, not a drag, until the pointer has moved a few pixels, and ignores targets that do not accept it', () => {
  const { row, onSlot } = setup();
  const clicked = vi.fn(); row.onclick = clicked;
  pointer('pointerdown', row, 0, 0); pointer('pointermove', document, 2, 1); pointer('pointerup', document, 2, 1); row.click();
  expect(document.querySelector('.drag-ghost')).toBeNull(); expect(onSlot).not.toHaveBeenCalled(); expect(clicked).toHaveBeenCalledOnce();
  const other = setup('fx');
  pointer('pointerdown', other.row, 0, 0); pointer('pointermove', document, 30, 0); pointer('pointerup', document, 30, 0);
  expect(other.onSlot).not.toHaveBeenCalled();
});

it('swallows the click that follows a drag', () => {
  const { row } = setup();
  const clicked = vi.fn(); row.onclick = clicked;
  pointer('pointerdown', row, 0, 0); pointer('pointermove', document, 30, 0); pointer('pointerup', document, 30, 0); row.click();
  expect(clicked).not.toHaveBeenCalled();
});
