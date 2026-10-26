// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';

// The app against a backend that answers like the engine does, so a mistake in what it does with a
// snapshot shows up as the window going offline or stopping.
const snapshot = {
  connected: true,
  revision: 1,
  transport: { sample: 96000, seconds: 2, bar: 2, beat: 1.5, bpm: 128, voices: 1, pending: 1 },
  beats_per_bar: 4,
  sample_rate: 48000,
  channels: 2,
  clips: [
    {
      name: 'beat',
      instrument: 'drums',
      running: true,
      muted: false,
      soloed: false,
      edits: ['reverse'],
      length: 1,
      looped: true,
      endless: true,
      cycles: 1.25,
      lanes: [
        {
          name: 'notes',
          truncated: false,
          events: [{ start: 0, dur: 0.0625, value: 'sample #0', gain: null, pan: null, src: [10, 11], eval: 1 }],
        },
      ],
    },
  ],
  scenes: [{ name: 'verse', clips: ['beat'], running: 1 }],
  pending: [{ id: 3, lands_at: 144000, in_samples: 48000, position: { bar: 3, beat: 1 }, changes: ['beat.reverse'] }],
  landed: [{ id: 2, at: 90000, position: { bar: 2, beat: 1 }, changes: ['beat.start'] }],
  notices: ['a notice'],
  error: null,
};

const calls: string[] = [];
vi.mock('@tauri-apps/api/core', () => ({
  isTauri: () => true,
  invoke: async (command: string) => {
    calls.push(command);
    if (command === 'snapshot') return snapshot;
    if (command === 'lsp_message') return [];
    return undefined;
  },
}));

afterEach(() => vi.useRealTimers());

describe('the stage app', () => {
  it('stays connected and keeps counting while the engine answers', async () => {
    document.body.innerHTML = '<div id="app"></div>';
    await import('./main');
    await new Promise(resolve => setTimeout(resolve, 600));
    expect(document.getElementById('connection')!.textContent).toContain('engine connected');
    expect(document.getElementById('bar')!.textContent).toBe('2');
    expect(calls.filter(c => c === 'snapshot').length).toBeGreaterThan(3);
    // it connects, but sends nothing until the player asks
    expect(calls).not.toContain('eval');
    console.log(document.getElementById('footer-status')!.textContent, document.getElementById('editor-status')!.textContent);
  });
});
