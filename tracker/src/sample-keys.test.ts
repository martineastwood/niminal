// @vitest-environment happy-dom
import { expect, it, vi } from 'vitest';
import { SampleKeys } from './sample-keys';

it('sounds a key while it is held and marks the keys that play pieces', () => {
  const play = vi.fn(), stop = vi.fn(), keys = new SampleKeys(play, stop);
  const key = (note: string) => keys.root.querySelector<HTMLButtonElement>(`[aria-label="${note}"]`)!;
  key('C-4').dispatchEvent(new Event('pointerdown'));
  expect(play).toHaveBeenCalledWith('C-4', 48);
  keys.root.dispatchEvent(new Event('pointerup'));
  expect(stop).toHaveBeenCalledWith(48);
  keys.mark(2);
  expect([...keys.root.querySelectorAll('.mapped')].map(k => k.getAttribute('aria-label'))).toEqual(['C-4', 'C#4']);
});
