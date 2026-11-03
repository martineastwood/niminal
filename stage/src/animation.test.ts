import { expect, it } from 'vitest';
import { elapsedSince, positionAt } from './animation';
import type { Snapshot } from './live';

function snapshot(bar: number, beat: number, bpm = 120, beats = 4): Snapshot {
  return { transport: { bar, beat, bpm }, beats_per_bar: beats } as Snapshot;
}

it('advances between snapshots and wraps bars in quarter-note beats, including 6/8', () => {
  expect(positionAt(snapshot(2, 4.9), 0.1)?.bar).toBe(3);
  expect(positionAt(snapshot(2, 4.9), 0.1)?.beat).toBeCloseTo(1.1);
  expect(positionAt(snapshot(2, 3.9, 120, 3), 0.1)?.bar).toBe(3);
  expect(positionAt(snapshot(2, 3.9, 120, 3), 0.1)?.beat).toBeCloseTo(1.1);
});

it('anchors each estimate to the latest tempo and position, including a clock reset', () => {
  expect(positionAt(snapshot(20, 2), 0.1)?.beat).toBeCloseTo(2.2);
  expect(positionAt(snapshot(1, 1, 60), 0.1)).toEqual({ bar: 1, beat: 1.1 });
  expect(positionAt({ transport: null } as Snapshot, 0.1)).toBeNull();
});

it('freezes after delayed snapshots or a background pause instead of inventing playback', () => {
  expect(elapsedSince(1000, 1100)).toBeCloseTo(0.1);
  expect(elapsedSince(1000, 1300)).toBe(0.25);
  expect(elapsedSince(1000, 61000)).toBe(0.25);
  expect(elapsedSince(1000, 900)).toBe(0);
});
