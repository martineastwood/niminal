import { describe, expect, it } from 'vitest';
import { blockAround, nextStatement, paragraphAround } from '../../editors/codemirror/blocks';

const lines = (text: string) => text.split('\n');

describe('the statement under the cursor', () => {
  it('is just the line when neighbours are separate statements', () => {
    const src = lines('tempo 100bpm\n\nmelody.start\nbass.start\n\nhush');
    expect(blockAround(src, 2)).toEqual({ start: 2, end: 2 });
    expect(blockAround(src, 3)).toEqual({ start: 3, end: 3 });
    expect(blockAround(src, 0)).toEqual({ start: 0, end: 0 });
    expect(blockAround(src, 1)).toBeNull();
  });

  it('includes .method and indented continuation lines', () => {
    const src = lines('a.start\nb\n  .reverse()\n  .fast(2)\nc.start');
    expect(blockAround(src, 1)).toEqual({ start: 1, end: 3 });
    expect(blockAround(src, 3)).toEqual({ start: 1, end: 3 });
    expect(blockAround(src, 4)).toEqual({ start: 4, end: 4 });
  });

  it('is the whole instrument from any line inside it, blank lines or not', () => {
    const src = lines('instr a(freq: hz) {\n  x = 1\n\n  g = grid {\n    1: x...\n  }.over(1bar)\n\n  out = x\n}\n\nhush');
    for (const line of [0, 1, 2, 4, 5, 6, 7, 8]) expect(blockAround(src, line)).toEqual({ start: 0, end: 8 });
    expect(blockAround(src, 9)).toBeNull();
    expect(blockAround(src, 10)).toEqual({ start: 10, end: 10 });
  });

  it('ignores brackets in comments and strings', () => {
    expect(blockAround(lines('a.start // [\nb.start\n'), 1)).toEqual({ start: 1, end: 1 });
    expect(blockAround(lines('a.load("x(.wav")\nb.start'), 1)).toEqual({ start: 1, end: 1 });
  });
});

describe('the paragraph under the cursor', () => {
  it('is the lines around it up to blank lines', () => {
    const src = lines('tempo 100bpm\n\nmelody.start\nbass.start\n\nhush');
    expect(paragraphAround(src, 3)).toEqual({ start: 2, end: 3 });
    expect(paragraphAround(src, 1)).toBeNull();
  });

  it('is not cut by blank lines inside brackets', () => {
    const src = lines('instr a(freq: hz) {\n  x = 1\n\n  out = x\n}\n\nhush');
    expect(paragraphAround(src, 3)).toEqual({ start: 0, end: 4 });
  });
});

describe('the next statement', () => {
  it('starts after the current one, skipping blank lines, comments and continuations', () => {
    const src = lines('a.start\n\n// note\nb\n  .fast(2)\nc.start');
    expect(nextStatement(src, 0)).toBe(3);
    expect(nextStatement(src, 3)).toBe(5);
    expect(nextStatement(src, 4)).toBe(5);
    expect(nextStatement(src, 5)).toBeNull();
  });
});
