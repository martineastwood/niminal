// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { Bar, fraction } from './meters';

describe('level meters', () => {
  it('puts 0 dB at the top and the floor at the bottom', () => {
    expect(fraction(1)).toBe(1);
    expect(fraction(0)).toBe(0);
    expect(fraction(0.001)).toBe(0);
    expect(fraction(0.1)).toBeCloseTo(2 / 3);
  });

  it('jumps up to a peak, falls slowly, and holds the peak tick a while', () => {
    const bar = new Bar();
    bar.push(0.5);
    const peak = bar.level;
    bar.step(0.2);
    expect(bar.level).toBeLessThan(peak);
    expect(bar.hold).toBe(peak);
    bar.step(2);
    expect(bar.hold).toBeLessThan(peak);
  });

  it('keeps the clip light on until it is cleared', () => {
    const bar = new Bar();
    bar.push(1.2);
    bar.step(5);
    expect(bar.clipped).toBe(true);
    bar.reset();
    expect(bar.clipped).toBe(false);
  });
});
