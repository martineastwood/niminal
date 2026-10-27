import { describe, expect, it } from 'vitest';
import { Text } from '@codemirror/state';
import { semanticMarks } from './semantic';

describe('semantic colours', () => {
  it('places each token by its line and column offsets', () => {
    const doc = Text.of(['fn f(x) {', '  y = x', '}']);
    // `x` (parameter) at 0:5, then `y` at 1:2 and `x` at 1:6, all parameters
    const marks = semanticMarks(doc, [0, 5, 1, 3, 0, 1, 2, 1, 3, 0, 0, 4, 1, 3, 0]);
    const found: [number, number, string][] = [];
    for (const cursor = marks.iter(); cursor.value; cursor.next()) {
      found.push([cursor.from, cursor.to, cursor.value.spec.class]);
    }
    expect(found).toEqual([
      [5, 6, 'cm-sem-parameter'],
      [12, 13, 'cm-sem-parameter'],
      [16, 17, 'cm-sem-parameter'],
    ]);
  });

  it('ignores a token past the end of the text', () => {
    expect(semanticMarks(Text.of(['a']), [4, 0, 1, 3, 0]).size).toBe(0);
  });
});
