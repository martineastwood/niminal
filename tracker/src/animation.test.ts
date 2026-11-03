import { afterEach, expect, it, vi } from 'vitest';
import { DisplayClock } from './animation';

afterEach(() => vi.restoreAllMocks());

it('advances on a monotonic clock, compensates for receipt latency and freezes on stale status', () => {
  vi.spyOn(Date, 'now').mockReturnValue(1050);
  const now = vi.spyOn(performance, 'now').mockReturnValue(10);
  const clock = new DisplayClock(); clock.anchor(2, 1000);
  expect(clock.read()).toBeCloseTo(2.05);
  now.mockReturnValue(110); expect(clock.read()).toBeCloseTo(2.15);
  now.mockReturnValue(1010); expect(clock.read()).toBe(2.25);
  // A system clock adjustment does not move an already anchored display.
  vi.mocked(Date.now).mockReturnValue(50000);
  expect(clock.read()).toBe(2.25);
});

it('re-anchors immediately at pattern changes and clamps negative receipt latency', () => {
  vi.spyOn(Date, 'now').mockReturnValue(1000);
  vi.spyOn(performance, 'now').mockReturnValue(10);
  const clock = new DisplayClock(); clock.anchor(10, 1000);
  clock.anchor(0.1, 1100); expect(clock.read()).toBe(0.1);
});
