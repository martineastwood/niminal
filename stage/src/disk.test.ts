// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tabsKey } from './tabs';

// A file open in the stage can be changed by another program (the VS Code extension's editor, say). The
// stage follows it unless the tab has edits of its own, and never replaces it without asking.
let disk = 'one';
const calls: string[] = [];
vi.mock('@tauri-apps/api/core', () => ({
  isTauri: () => true,
  invoke: async (command: string, args?: Record<string, any>) => {
    calls.push(command);
    if (command === 'open_path') return { path: args?.path, source: disk };
    if (command === 'snapshot') return { connected: false };
    if (command === 'lsp_message') return [];
    if (command === 'confirm') return false;
    return undefined;
  },
}));

const path = '/songs/a.nml';
const editorText = () => document.querySelector('.cm-content')?.textContent ?? '';
const status = () => document.getElementById('footer-status')?.textContent ?? '';

async function openWith(text: string, saved: string) {
  localStorage.setItem(tabsKey, JSON.stringify({ tabs: [{ path, title: 'a.nml', text, saved }], active: 0 }));
  document.body.innerHTML = '<div id="app"></div>';
  vi.resetModules();
  await import('./main');
  await vi.advanceTimersByTimeAsync(10);
}

beforeEach(() => {
  vi.useFakeTimers();
  calls.length = 0;
  localStorage.clear();
});
afterEach(() => vi.useRealTimers());

describe('a file that changes on disk', () => {
  it('is followed by a tab with no edits of its own', async () => {
    disk = 'one';
    await openWith('one', 'one');
    expect(editorText()).toContain('one');
    disk = 'two';
    await vi.advanceTimersByTimeAsync(1600);
    expect(editorText()).toContain('two');
    expect(status()).toContain('reloaded');
  });

  it('leaves a tab with edits alone, and says so', async () => {
    disk = 'one';
    await openWith('my edit', 'one');
    disk = 'two';
    await vi.advanceTimersByTimeAsync(1600);
    expect(editorText()).toContain('my edit');
    expect(editorText()).not.toContain('two');
    expect(status()).toContain('changed on disk');
  });

  it('is not replaced by saving unless the answer is yes', async () => {
    disk = 'one';
    await openWith('my edit', 'one');
    disk = 'two';
    document.getElementById('save')!.click();
    await vi.advanceTimersByTimeAsync(10);
    expect(calls).toContain('confirm');
    expect(calls).not.toContain('save_file');
  });
});
