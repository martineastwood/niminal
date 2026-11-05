// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';

// Choosing the sound card starts the engine again, so the window asks first and says what is lost.
const calls: { command: string; args?: Record<string, any> }[] = [];
let agree = true;
const state = {
  devices: [
    { name: 'MacBook Pro Speakers', rates: [44100, 48000] },
    { name: 'Scarlett 2i2', rates: [44100, 48000, 96000] },
  ],
  choice: { output: null, buffer_size: null, sample_rate: null },
  sample_rate: 44100,
  output: 'MacBook Pro Speakers',
  osc_port: null,
};
vi.mock('@tauri-apps/api/core', () => ({
  isTauri: () => true,
  invoke: async (command: string, args?: Record<string, any>) => {
    calls.push({ command, args });
    if (command === 'snapshot') return { connected: true, transport: null, clips: [], scenes: [], pending: [], landed: [], notices: [], master: [], levels: { nodes: {}, buses: {} }, health: null, learned: [], midi: { inputs: [], last: null }, tools_problem: null };
    if (command === 'audio_state') return state;
    if (command === 'audio_apply') return { ...state, sample_rate: args?.sampleRate ?? 44100, output: args?.output ?? state.output };
    if (command === 'confirm') return agree;
    if (command === 'lsp_message') return [];
    return undefined;
  },
}));

const select = (id: string) => document.getElementById(id) as HTMLSelectElement;
const names = (id: string) => [...select(id).options].map(o => o.textContent);

async function open() {
  document.body.innerHTML = '<div id="app"></div>';
  vi.resetModules();
  await import('./main');
  await new Promise(r => setTimeout(r, 20));
  window.dispatchEvent(new KeyboardEvent('keydown', { key: ',', metaKey: true }));
  await new Promise(r => setTimeout(r, 20));
}

beforeEach(() => {
  calls.length = 0;
  agree = true;
  localStorage.clear();
});

describe('the audio settings', () => {
  it('lists the sound cards and offers the sample rates of the one chosen', async () => {
    await open();
    expect(document.getElementById('settings')!.hidden).toBe(false);
    expect(names('audio-output')).toEqual(['system default', 'MacBook Pro Speakers', 'Scarlett 2i2']);
    select('audio-output').value = 'Scarlett 2i2';
    select('audio-output').dispatchEvent(new Event('change'));
    await new Promise(r => setTimeout(r, 20));
    expect(names('audio-rate')).toContain('96000 Hz');
  });

  it('starts the engine again on what was chosen, once the player has agreed', async () => {
    await open();
    select('audio-output').value = 'Scarlett 2i2';
    select('audio-rate').value = '';
    select('audio-buffer').value = '128';
    document.getElementById('audio-apply')!.click();
    await new Promise(r => setTimeout(r, 20));
    const apply = calls.find(c => c.command === 'audio_apply');
    expect(apply?.args).toEqual({ output: 'Scarlett 2i2', bufferSize: 128, sampleRate: null });
  });

  it('does nothing if the player says no', async () => {
    await open();
    agree = false;
    document.getElementById('audio-apply')!.click();
    await new Promise(r => setTimeout(r, 20));
    expect(calls.some(c => c.command === 'audio_apply')).toBe(false);
  });

  it('listens for OSC on the port given, and stops when it is empty', async () => {
    await open();
    (document.getElementById('audio-osc') as HTMLInputElement).value = '9000';
    document.getElementById('osc-apply')!.click();
    await new Promise(r => setTimeout(r, 20));
    expect(calls.find(c => c.command === 'osc_listen')?.args).toEqual({ port: 9000 });
    (document.getElementById('audio-osc') as HTMLInputElement).value = '';
    document.getElementById('osc-apply')!.click();
    await new Promise(r => setTimeout(r, 20));
    expect(calls.filter(c => c.command === 'osc_listen').at(-1)?.args).toEqual({ port: null });
  });
});
