// @vitest-environment happy-dom
import { EditorView } from '@codemirror/view';
import { afterEach, describe, expect, it, vi } from 'vitest';

// The app against a backend that answers like the engine does, so a mistake in what it does with a
// snapshot shows up as the window going offline or stopping.
const snapshot = {
  connected: true,
  revision: 1,
  transport: { sample: 96000, seconds: 2, bar: 2, beat: 1.5, bpm: 128, voices: 1, pending: 1 },
  beats_per_bar: 4,
  meter: [4, 4],
  sample_rate: 48000,
  channels: 2,
  clips: [
    {
      name: 'lead',
      instrument: 'pluck',
      running: false,
      muted: false,
      soloed: false,
      edits: [],
      length: 1,
      looped: true,
      endless: true,
      cycles: null,
      lanes: [],
    },
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
  scenes: [
    { name: 'verse', clips: ['beat'], running: 1 },
    { name: 'chorus', clips: ['lead'], running: 0 },
  ],
  pending: [{ id: 3, lands_at: 144000, in_samples: 48000, position: { bar: 3, beat: 1 }, changes: ['beat.reverse'] }],
  landed: [{ id: 2, at: 90000, position: { bar: 2, beat: 1 }, changes: ['beat.start'] }],
  notices: ['a notice'],
  error: null,
};

const calls: string[] = [];
const sent: Record<string, any>[] = [];
vi.mock('@tauri-apps/api/core', () => ({
  isTauri: () => true,
  invoke: async (command: string, args?: Record<string, any>) => {
    calls.push(command);
    if (command === 'eval') sent.push(args ?? {});
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
    expect(document.getElementById('connection')!.textContent).toContain('engine');
    expect(document.getElementById('bar')!.textContent).toBe('2');
    expect(calls.filter(c => c === 'snapshot').length).toBeGreaterThan(3);
    // it connects, but sends nothing until the player asks
    expect(calls).not.toContain('eval');
    console.log(document.getElementById('footer-status')!.textContent, document.getElementById('editor-status')!.textContent);
  });

  it('changes the text size, remembers it, and keeps the mode controls working', async () => {
    window.dispatchEvent(new KeyboardEvent('keydown', { key: '=', metaKey: true }));
    expect(document.documentElement.style.getPropertyValue('--z')).toBe('1.15');
    expect(localStorage.getItem('niminal-stage-zoom')).toBe('1.15');
    window.dispatchEvent(new KeyboardEvent('keydown', { key: '-', metaKey: true }));
    window.dispatchEvent(new KeyboardEvent('keydown', { key: '-', metaKey: true }));
    expect(document.documentElement.style.getPropertyValue('--z')).toBe('0.85');
    window.dispatchEvent(new KeyboardEvent('keydown', { key: '0', metaKey: true }));
    expect(document.documentElement.style.getPropertyValue('--z')).toBe('1');

    document.getElementById('perform')!.click();
    expect(document.body.classList.contains('perform')).toBe(true);
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'P', metaKey: true, shiftKey: true }));
    expect(document.body.classList.contains('perform')).toBe(false);
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'b', metaKey: true }));
    expect(document.body.classList.contains('no-rail')).toBe(true);
  });

  it('starts the nth scene from the keyboard, which the engine switches to', async () => {
    window.dispatchEvent(new KeyboardEvent('keydown', { key: '2', metaKey: true }));
    await new Promise(resolve => setTimeout(resolve, 50));
    expect(sent.at(-1)!.source).toBe('chorus.start');
  });

  it('stops a scene that is already playing when it is pressed again', async () => {
    window.dispatchEvent(new KeyboardEvent('keydown', { key: '1', metaKey: true }));
    await new Promise(resolve => setTimeout(resolve, 50));
    expect(sent.at(-1)!.source).toBe('verse.stop');
  });

  it('keeps each tab\'s text apart and evaluates them left to right', async () => {
    const tabs = () => [...document.querySelectorAll('.tab')].map(t => t.querySelector('.tab-name')!.textContent);
    const first = tabs().length;
    document.getElementById('new')!.click();
    expect(tabs().length).toBe(first + 1);
    expect(document.getElementById('evaluate-all')!.hidden).toBe(false);
    const editor = EditorView.findFromDOM(document.querySelector<HTMLElement>('.cm-editor')!)!;
    editor.dispatch({ changes: { from: 0, insert: 'bus a: stereo' } });
    expect(document.querySelector('.tab.active')!.classList.contains('dirty')).toBe(true);

    document.querySelector<HTMLElement>('.tab:not(.active)')!.click();
    expect(editor.state.doc.toString()).not.toContain('bus a: stereo');

    sent.length = 0;
    document.getElementById('evaluate-all')!.click();
    await new Promise(resolve => setTimeout(resolve, 100));
    expect(sent.length).toBe(first + 1);
    expect(sent.at(-1)!.source).toBe('bus a: stereo');
  });

  it('moves a tab to where it is dropped', async () => {
    const names = () => [...document.querySelectorAll('.tab-name')].map(t => t.textContent);
    const before = names();
    const last = before.length - 1;
    const items = () => [...document.querySelectorAll<HTMLElement>('.tab')];
    items()[last].dispatchEvent(new Event('dragstart', { bubbles: true }));
    items()[0].getBoundingClientRect = () => ({ left: 0, width: 100 }) as DOMRect;
    items()[0].dispatchEvent(Object.assign(new Event('drop', { bubbles: true, cancelable: true }), { clientX: 10 }));
    expect(names()).toEqual([before[last], ...before.slice(0, last)]);
  });
});
