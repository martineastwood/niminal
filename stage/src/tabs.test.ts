import { describe, expect, it } from 'vitest';
import { moveItem } from './tabs';

describe('moveItem', () => {
  const moved = (from: number, before: number) => {
    const items = ['a', 'b', 'c', 'd'];
    moveItem(items, from, before);
    return items.join('');
  };
  it('puts an item before another, forwards or backwards', () => {
    expect(moved(0, 3)).toBe('bcad');
    expect(moved(3, 1)).toBe('adbc');
    expect(moved(1, 4)).toBe('acdb');
    expect(moved(1, 1)).toBe('abcd');
    expect(moved(1, 2)).toBe('abcd');
  });
});
