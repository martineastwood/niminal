// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { LaneView } from './laneview';
import { Clip, Snapshot } from './live';

const clip: Clip = {
  name: 'beat',
  instrument: 'drums',
  running: true,
  muted: false,
  soloed: false,
  edits: ['reverse', 'drop(25%)', 'reverse'],
  length: 1,
  looped: true,
  endless: true,
  cycles: 0.5,
  lanes: [
    {
      name: 'notes',
      truncated: false,
      events: [{ start: 0, dur: 0.25, value: 'c4', gain: null, pan: null, src: null, eval: null }],
    },
  ],
};

const snapshot = (): Snapshot =>
  ({ connected: true, clips: [clip], scenes: [], pending: [], landed: [], notices: [], transport: null }) as unknown as Snapshot;

describe('a clip in the sidebar', () => {
  it('is one line with the changes made to it in order, the newest marked', () => {
    const el = document.createElement('div');
    new LaneView(el, () => {}).update(snapshot());
    const chips = [...el.querySelectorAll('.chip')].map(c => c.textContent);
    expect(chips).toEqual(['reverse', 'drop(25%)', 'reverse']);
    expect(el.querySelectorAll('.chip.newest').length).toBe(1);
    expect(el.querySelector('.clip-lanes')).toBeNull();
    expect(el.querySelector('.undo-stack')).toBeNull();
  });

  it('opens to show the order the changes come back in, and can undo them', () => {
    const el = document.createElement('div');
    const actions: unknown[][] = [];
    const view = new LaneView(el, (...args) => actions.push(args));
    view.update(snapshot());
    el.querySelector<HTMLElement>('.clip-head')!.click();
    const lines = [...el.querySelectorAll('.undo-line')].map(l => l.textContent);
    // newest first, then what it was written as
    expect(lines[0]).toContain('reverse');
    expect(lines[0]).toContain('next undo');
    expect(lines[1]).toContain('drop(25%)');
    expect(lines[3]).toBe('as written');
    expect(el.querySelector('.clip-lanes')).not.toBeNull();
    el.querySelector<HTMLButtonElement>('.undo-actions button')!.click();
    expect(actions).toEqual([['edit', 'beat', 'undo']]);
    // clicking again closes it
    el.querySelector<HTMLElement>('.clip-head')!.click();
    expect(el.querySelector('.undo-stack')).toBeNull();
  });
});
