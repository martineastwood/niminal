// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { LaneView } from './laneview';
import { Clip, Snapshot } from './live';

function snapshot(overrides: Partial<Snapshot>): Snapshot {
  return {
    connected: true,
    revision: 0,
    transport: null,
    beats_per_bar: 4,
    sample_rate: 48000,
    channels: 2,
    clips: [],
    scenes: [],
    pending: [],
    landed: [],
    notices: [],
    error: null,
    ...overrides,
  };
}

const clip = (running: boolean): Clip => ({
  name: 'beat',
  instrument: 'drums',
  running,
  muted: false,
  soloed: false,
  edits: [],
  length: 1,
  looped: true,
  endless: true,
  cycles: running ? 0 : null,
  lanes: [],
});

describe('the ready screen', () => {
  const root = () => document.createElement('div');

  it('says there is no engine, and offers to connect', () => {
    const el = root();
    const actions: string[] = [];
    new LaneView(el, action => actions.push(action)).update(snapshot({ connected: false }));
    expect(el.textContent).toContain('No engine yet');
    el.querySelector<HTMLButtonElement>('button[data-action="connect"]')!.click();
    expect(actions).toEqual(['connect']);
  });

  it('teaches the shortcuts when nothing has been sent', () => {
    const el = root();
    new LaneView(el, () => {}).update(snapshot({}));
    expect(el.textContent).toContain('Ready');
    expect(el.querySelectorAll('.shortcut-list dt').length).toBeGreaterThan(4);
  });

  it('says what to press when clips exist but none is playing', () => {
    const el = root();
    const view = new LaneView(el, () => {});
    view.update(snapshot({ clips: [clip(false)], scenes: [{ name: 'verse', clips: ['beat'], running: 0 }] }));
    expect(el.querySelector('.stage-ready')!.textContent).toContain('verse');
    view.update(snapshot({ clips: [clip(true)], scenes: [{ name: 'verse', clips: ['beat'], running: 1 }] }));
    expect(el.querySelector('.stage-ready')).toBeNull();
  });
});
