import { describe, expect, it } from 'vitest';
import { families, summary } from './gallery';

describe('gallery', () => {
  it('names the settings a version was made from, shortest first', () => {
    expect(summary({ density: 15.111, grain: 0.2468, spray: 0.232, scatter: 3.7 })).toBe(
      'density 15.1 · grain 0.247 · spray 0.232',
    );
    expect(summary({})).toBe('');
    expect(summary({ count: 4 }, 3)).toBe('count 4');
  });

  it('knows the families the command makes', () => {
    expect(families).toEqual(['players', 'manglers', 'glitch', 'synthesis', 'modifiers', 'replicators']);
  });
});
