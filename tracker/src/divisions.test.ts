import { describe, expect, it } from 'vitest';
import { divisions, nearestDivision } from './divisions';

describe('divisions', () => {
  it('are listed from shortest to longest', () => {
    expect(divisions.map(d => d.beats)).toEqual([...divisions.map(d => d.beats)].sort((a, b) => a - b));
    expect(divisions[0].name).toBe('1/32');
    expect(divisions.at(-1)!.name).toBe('1/1');
  });
  it('are named for the note they are', () => {
    expect(nearestDivision(0.5).name).toBe('1/8');
    expect(nearestDivision(1 / 3).name).toBe('1/8t');
    expect(nearestDivision(0.75).name).toBe('1/8d');
  });
  it('snap a length in between to the nearest one by ratio', () => {
    expect(nearestDivision(0.52).name).toBe('1/8');
    expect(nearestDivision(0.62).name).toBe('1/4t');
    expect(nearestDivision(100).name).toBe('1/1');
  });
});
