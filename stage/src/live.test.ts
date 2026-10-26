import { describe, expect, it } from 'vitest';
import { Clip, isSounding, phaseOf, sinceOnset, windowOf } from './live';

function clip(overrides: Partial<Clip> = {}): Clip {
  return {
    name: 'riff',
    instrument: 'pluck',
    running: true,
    muted: false,
    soloed: false,
    edits: [],
    length: 1,
    looped: true,
    endless: false,
    cycles: 0,
    lanes: [],
    ...overrides,
  };
}

describe('the clip window', () => {
  it('is the clip length, or one cycle when it is endless', () => {
    expect(windowOf(clip({ length: 2 }))).toBe(2);
    expect(windowOf(clip({ endless: true, length: 4 }))).toBe(1);
  });

  it('wraps within the loop and keeps fractional cycles', () => {
    const loop = clip({ length: 2 });
    expect(phaseOf(loop, 0.25)).toBeCloseTo(0.25);
    expect(phaseOf(loop, 2.25)).toBeCloseTo(0.25);
    expect(phaseOf(loop, 4.75)).toBeCloseTo(0.75);
  });

  it('shows one cycle at a time for endless clips', () => {
    const endless = clip({ endless: true, length: 1 });
    expect(phaseOf(endless, 0.4)).toBeCloseTo(0.4);
    expect(phaseOf(endless, 5.4)).toBeCloseTo(0.4);
  });
});

describe('a sounding note', () => {
  const loop = clip();

  it('is lit from its onset until its length runs out', () => {
    expect(isSounding(loop, { start: 0.25, dur: 0.5 }, 0.24)).toBe(false);
    expect(isSounding(loop, { start: 0.25, dur: 0.5 }, 0.25)).toBe(true);
    expect(isSounding(loop, { start: 0.25, dur: 0.5 }, 0.74)).toBe(true);
    expect(isSounding(loop, { start: 0.25, dur: 0.5 }, 0.75)).toBe(false);
  });

  it('flashes an instant note long enough to see', () => {
    expect(isSounding(loop, { start: 0.5, dur: 0 }, 0.5)).toBe(true);
    expect(isSounding(loop, { start: 0.5, dur: 0 }, 0.55)).toBe(true);
    expect(isSounding(loop, { start: 0.5, dur: 0 }, 0.57)).toBe(false);
  });
});

describe('an endless clip across a cycle boundary', () => {
  // read at 2.9 cycles: its lane is cycle 2, which plays 1 2 3 4; cycle 3 plays them backwards
  const endless = clip({ endless: true, length: 1, cycles: 2.9 });
  const notes = [0, 0.25, 0.5, 0.75].map(start => ({ start, dur: 0.25 }));

  it('does not light the first note of the old cycle when the next one begins', () => {
    // 3.05 cycles in: cycle 2's notes are long over, and cycle 3's are not known yet
    expect(notes.map(n => isSounding(endless, n, 3.05))).toEqual([false, false, false, false]);
  });

  it('lets the last note of the old cycle keep fading past the boundary', () => {
    expect(sinceOnset(endless, notes[3], 3.05)).toBeCloseTo(0.3);
    expect(isSounding(endless, notes[3], 2.99)).toBe(true);
  });
});
