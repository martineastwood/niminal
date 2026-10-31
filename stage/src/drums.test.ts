import { describe, expect, it } from 'vitest';
import { auditionDrums, drumCall, gridCode } from './drums';

const base = { theme: 'house', density: 0.8, chaos: 0, seed: 4, scales: {} };

describe('drum generator code', () => {
  it('writes the call that makes the grid, leaving out what is at its default', () => {
    expect(drumCall(base)).toBe('drum_grid(theme: house, density: 0.8, seed: 4)');
    expect(drumCall({ ...base, chaos: 0.3, scales: { hat: 0.6, kick: 1 } })).toBe(
      'drum_grid(theme: house, density: 0.8, chaos: 0.3, seed: 4, hat: 0.6)',
    );
  });

  it('writes the rows out as a grid in groups of four', () => {
    const rows = [
      { name: 'kick', steps: 'x...x...x...x...' },
      { name: 'open', steps: '..x...x...x...x.' },
    ];
    expect(gridCode(rows)).toBe('grid {\n  kick: x... x... x... x...\n  open: ..x. ..x. ..x. ..x.\n}');
  });

  it('auditions through a kit of the shelf drums under a name of its own', () => {
    const code = auditionDrums(base);
    expect(code).toContain('use std/drums.');
    expect(code).toContain('notes: drum_grid(theme: house, density: 0.8, seed: 4)');
    expect(code.trimEnd().endsWith('__lab_drums.start')).toBe(true);
  });
});
