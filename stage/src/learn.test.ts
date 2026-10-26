import { describe, expect, it } from 'vitest';
import { learnedRange } from './learn';

describe('writing back what a control learned', () => {
  const doc = 'ctl volume = midi.cc(7)\nctl cutoff = midi.learn.range(200hz..4khz)\nctl res = midi.learn()';

  it('finds the learn on the control line, leaving the rest of the chain', () => {
    const range = learnedRange(doc, 'cutoff')!;
    expect(doc.slice(range.from, range.to)).toBe('midi.learn');
  });

  it('takes the call parentheses too', () => {
    const range = learnedRange(doc, 'res')!;
    expect(doc.slice(range.from, range.to)).toBe('midi.learn()');
  });

  it('finds nothing for a control that is not waiting', () => {
    expect(learnedRange(doc, 'volume')).toBeNull();
    expect(learnedRange(doc, 'vol')).toBeNull();
  });
});
