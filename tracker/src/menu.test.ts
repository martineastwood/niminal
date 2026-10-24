// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { openMenu } from './menu';

afterEach(() => document.body.replaceChildren());

it('offers a number with a button, runs it only when it is in range, and closes', () => {
  const anchor = document.createElement('button'); document.body.append(anchor);
  const run = vi.fn(), item = vi.fn();
  openMenu(anchor, [{ heading: 'Cut' }, { text: 'Four', run: item }, { number: 'Pieces', value: 12, min: 2, max: 48, run }]);
  const input = document.querySelector<HTMLInputElement>('[aria-label=Pieces]')!;
  expect(input.value).toBe('12');
  input.value = '99'; document.querySelector<HTMLButtonElement>('.menu-number button')!.click();
  expect(run).not.toHaveBeenCalled(); expect(document.querySelector('.menu')).not.toBeNull();
  input.value = '7'; input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
  expect(run).toHaveBeenCalledWith(7); expect(document.querySelector('.menu')).toBeNull();
});
