import { describe, expect, it } from 'vitest';
import { blockAround } from '../../editors/codemirror/blocks';

const lines = (text: string) => text.split('\n');

describe('the block under the cursor', () => {
  it('is the lines around it up to blank lines', () => {
    const src = lines('tempo 100bpm\n\na.start\nb.reverse\n\nhush');
    expect(blockAround(src, 3)).toEqual({ start: 2, end: 3 });
    expect(blockAround(src, 0)).toEqual({ start: 0, end: 0 });
    expect(blockAround(src, 1)).toBeNull();
  });

  it('is not cut by blank lines inside brackets', () => {
    const src = lines('instr a(freq: hz) {\n  x = 1\n\n  out = x\n}\n\nhush');
    expect(blockAround(src, 1)).toEqual({ start: 0, end: 4 });
    expect(blockAround(src, 3)).toEqual({ start: 0, end: 4 });
  });

  it('ignores brackets in comments', () => {
    expect(blockAround(lines('a.start // [\n\nhush'), 0)).toEqual({ start: 0, end: 0 });
  });
});
