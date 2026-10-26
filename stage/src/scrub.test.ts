// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { numberAt, nudge } from './scrub';

describe('finding a number', () => {
  const at = (line: string, column: number) => {
    const hit = numberAt(line, column);
    return hit && line.slice(hit.from, hit.end);
  };

  it('takes the number under the cursor with its unit', () => {
    expect(at('cutoff: 440hz', 10)).toBe('440hz');
    expect(at('cutoff: 440hz', 12)).toBe('440hz');
  });

  it('takes the next number when the cursor is not on one', () => {
    expect(at('gain: -6db', 0)).toBe('-6db');
  });

  it('does not take note names, names with digits, or comments', () => {
    expect(at('notes: [c4 e4]', 3)).toBeNull();
    expect(at('clip kick2 // 120bpm', 0)).toBeNull();
  });

  it('keeps a subtraction as a subtraction', () => {
    expect(at('x = a-1', 0)).toBe('1');
  });
});

describe('moving a number', () => {
  it('moves by its last digit', () => {
    expect(nudge('0.5', 1)).toBe('0.6');
    expect(nudge('3', -1)).toBe('2');
    expect(nudge('0.25', 2)).toBe('0.27');
  });

  it('moves big numbers faster', () => {
    expect(nudge('440', 1)).toBe('441');
    expect(nudge('4400', 1)).toBe('4410');
  });

  it('crosses zero cleanly', () => {
    expect(nudge('1', -2)).toBe('-1');
    expect(nudge('0.1', -1)).toBe('0.0');
  });
});
