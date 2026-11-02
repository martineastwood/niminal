// @vitest-environment happy-dom
import { afterAll, expect, it, vi } from 'vitest';
import type { AudioStatus } from './audio';
import type { MidiMessage } from './midi';

const backend = vi.hoisted(() => ({
  status: {
    connected: false, playing: false, row: null, row_phase: null, row_at_ms: 0,
    pattern_id: null, sequence_position: null, sample_rate: 48000,
    skipped_rows: 0, late_events: 0, error: null,
    instruments: {}, effects: {}, instrument_parameters: {}, effect_parameters: {},
  } as AudioStatus,
  commands: [] as Record<string, any>[],
  midi: null as ((message: MidiMessage) => void) | null,
  connect: null as (() => void) | null,
  menu: null as ((event: { payload: string }) => void) | null,
}));
vi.mock('@tauri-apps/api/core', () => ({
  isTauri: () => true,
  invoke: async (name: string, args?: Record<string, any>) => {
    if (name === 'project_dir') return '/tmp/niminal-tracker-test';
    if (name === 'audio_scopes') return {};
    if (name === 'audio_status') return structuredClone(backend.status);
    if (name !== 'audio_command') return [];
    const command = args!.command;
    backend.commands.push(command);
    if (command.op === 'connect') {
      await new Promise<void>(resolve => { backend.connect = resolve; });
      backend.status.connected = true;
    }
    if (command.op === 'disconnect') backend.status.connected = false;
    if (command.op === 'apply' && command.kind === 'fx') {
      backend.status.effects[command.id] = 'fx';
      backend.status.effect_parameters[command.id] = [{ name: 'mix', min: 0, max: 1, default: 0.5, unit: null }];
    } else if (command.op === 'apply') {
      backend.status.instruments[command.id] = 'synth';
      backend.status.instrument_parameters[command.id] = [{ name: 'level', min: 0, max: 1, default: 0.5, unit: null }];
    }
    return structuredClone(backend.status);
  },
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: async (name: string, callback: typeof backend.menu) => {
    if (name === 'menu') backend.menu = callback;
    return () => {};
  },
}));
vi.mock('@tauri-apps/api/webview', () => ({ getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }) }));
vi.mock('./midi', async importOriginal => ({ ...await importOriginal<typeof import('./midi')>(), mountMidi: async (_parent: HTMLElement, receive: (message: MidiMessage) => void) => { backend.midi = receive; return () => {}; } }));
vi.mock('./editor', async importOriginal => ({
  ...await importOriginal<typeof import('./editor')>(), connectLanguageServer: async () => {},
}));

vi.stubGlobal('Option', function (text: string, value: string) {
  const option = document.createElement('option'); option.textContent = text; option.value = value; return option;
});
afterAll(() => window.dispatchEvent(new Event('beforeunload')));

it('shares startup with a new instrument, exposes its controls, and reconnects after losing the engine', async () => {
  document.body.innerHTML = '<div id="app"></div>';
  await import('./main');
  await vi.waitFor(() => expect(backend.connect).not.toBeNull());
  backend.menu!({ payload: 'new-instrument' });
  expect(backend.commands.filter(c => c.op === 'connect')).toHaveLength(1);
  backend.connect!();
  await vi.waitFor(() => expect(document.getElementById('instr-controls')!.textContent).toContain('level'));
  expect(document.getElementById('instr-controls')!.textContent).not.toContain('Parameters appear once the engine has applied this');
  expect(backend.commands.some(c => c.op === 'audition')).toBe(true);

  backend.status.connected = false;
  backend.status.instruments = {};
  backend.status.instrument_parameters = {};
  await vi.waitFor(() => expect(backend.commands.filter(c => c.op === 'connect')).toHaveLength(2), { timeout: 5000 });
  backend.connect!();
  await vi.waitFor(() => expect(document.getElementById('instr-status')!.textContent).toBe('Applied'));
  expect(document.getElementById('instr-controls')!.textContent).toContain('level');
});

it('leaves the cursor in the grid after Escape and after clicks on buttons and menus, unless focus went somewhere on purpose', async () => {
  const grid = document.getElementById('grid')!, active = () => document.activeElement;
  const record = () => document.getElementById('record')!.getAttribute('aria-pressed');
  // Escape from a text box turns record mode on and puts the cursor back in the grid
  const name = document.getElementById('velocity')!; name.focus();
  const before = record();
  name.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
  expect(record()).not.toBe(before);
  await vi.waitFor(() => expect(active()).toBe(grid));
  // a toolbar button leaves it there too
  const undo = document.getElementById('undo') as HTMLButtonElement; undo.focus(); undo.click();
  await vi.waitFor(() => expect(active()).toBe(grid));
  // a button of the sequence bar
  const first = document.querySelector<HTMLButtonElement>('#sequence .sequence-main')!; first.focus(); first.click();
  await vi.waitFor(() => expect(active()).toBe(grid));
  // a choice made in a menu of the toolbar
  const picker = document.getElementById('meter-unit') as HTMLSelectElement; picker.focus();
  picker.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true })); picker.dispatchEvent(new Event('change', { bubbles: true }));
  await vi.waitFor(() => expect(active()).toBe(grid));
  // but not when the button opened something that wants the keyboard
  const palette = document.getElementById('open-palette') as HTMLButtonElement; palette.focus(); palette.click();
  await new Promise(resolve => setTimeout(resolve, 20));
  expect(active()).toBe(document.querySelector('.palette input'));
  document.querySelector('.palette input')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  // nor from the code editor, which keeps its own focus on Escape
  const editor = document.createElement('div'); editor.className = 'cm-editor'; const content = document.createElement('div'); content.tabIndex = 0; editor.append(content); document.body.append(editor);
  content.focus(); content.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
  await new Promise(resolve => setTimeout(resolve, 20));
  expect(active()).toBe(content); editor.remove();
});

it('gives the keyboard back to the grid after Enter in a box, a tick box, a slider or a choice from a drop-down, but not while arrowing through one', async () => {
  const grid = document.getElementById('grid')!, active = () => document.activeElement, tick = () => new Promise(done => setTimeout(done, 20));
  const velocity = document.getElementById('velocity') as HTMLInputElement; velocity.focus();
  velocity.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
  await vi.waitFor(() => expect(active()).toBe(grid));
  const follow = document.getElementById('follow') as HTMLInputElement; follow.focus(); follow.click();
  await vi.waitFor(() => expect(active()).toBe(grid));
  const picker = document.getElementById('meter-unit') as HTMLSelectElement; picker.focus();
  picker.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true })); picker.dispatchEvent(new Event('change', { bubbles: true }));
  await tick(); expect(active()).toBe(picker); // moving through the options is not a choice
  picker.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); picker.dispatchEvent(new Event('change', { bubbles: true }));
  await vi.waitFor(() => expect(active()).toBe(grid));
  // a range slider after the pointer is let go
  const slider = document.createElement('input'); slider.type = 'range'; document.getElementById('app')!.append(slider); slider.focus();
  slider.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true })); slider.dispatchEvent(new PointerEvent('pointerup', { bubbles: true }));
  await vi.waitFor(() => expect(active()).toBe(grid)); slider.remove();
  // but a text box in the sound browser keeps it, for its own Enter
  const search = document.querySelector<HTMLInputElement>('#sound-browser input')!; search.focus();
  search.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
  await tick(); expect(active()).toBe(search);
});

/** Adds an effect the way the dock's list does: its row, double-clicked. */
function addFromList(name: string) {
  const search = document.querySelector<HTMLInputElement>('#fx-browser input')!; search.value = name; search.dispatchEvent(new Event('input'));
  document.querySelector<HTMLElement>('#fx-browser .sb-item')!.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
}

it('applies an effect as soon as it is added to a track, so its controls appear without pressing Apply', async () => {
  backend.commands.length = 0;
  addFromList('Echo');
  await vi.waitFor(() => expect(backend.commands.some(c => c.op === 'apply' && c.kind === 'fx')).toBe(true));
  await vi.waitFor(() => expect(document.getElementById('mixer')!.textContent).toContain('mix'));
  expect(document.getElementById('mixer')!.textContent).not.toContain('Parameters appear once the engine has applied this');
  await vi.waitFor(() => expect(document.getElementById('fx-status')!.textContent).toBe('Applied'));
  const apply = document.getElementById('fx-apply') as HTMLButtonElement;
  expect(apply.disabled).toBe(false); expect(apply.classList.contains('dirty')).toBe(false); // clickable, and not highlighted once applied
});

it('applies an effect added while the engine was not connected as soon as it connects', async () => {
  backend.status.connected = false; backend.status.effects = {}; backend.status.effect_parameters = {}; backend.commands.length = 0;
  await vi.waitFor(() => expect(backend.commands.some(c => c.op === 'connect')).toBe(true), { timeout: 5000 });
  addFromList('Lowpass');
  expect(backend.commands.some(c => c.op === 'apply' && c.kind === 'fx')).toBe(false);
  backend.connect!();
  await vi.waitFor(() => expect(backend.commands.filter(c => c.op === 'apply' && c.kind === 'fx').length).toBeGreaterThan(0));
  await vi.waitFor(() => expect(document.getElementById('fx-status')!.textContent).toBe('Applied'));
  expect(document.getElementById('mixer')!.textContent).toContain('mix');
});


it('holds MIDI preview under the pedal and Stop clears it before new input', async () => {
  const record = document.getElementById('record')!;
  if (record.getAttribute('aria-pressed') === 'true') record.click();
  const send = backend.midi!;
  send({ kind: 'reset' });
  await vi.waitFor(() => expect(backend.commands.some(c => c.op === 'midi_off' && c.key === null)).toBe(true));
  backend.commands.length = 0;
  send({ kind: 'on', channel: 0, note: 60, velocity: 100 });
  await vi.waitFor(() => expect(backend.commands.some(c => c.op === 'midi_on' && c.key === 60)).toBe(true));
  send({ kind: 'sustain', channel: 0, down: true }); send({ kind: 'off', channel: 0, note: 60 });
  await new Promise(resolve => setTimeout(resolve, 20));
  expect(backend.commands.some(c => c.op === 'midi_off' && c.key === 60)).toBe(false);
  send({ kind: 'sustain', channel: 0, down: false });
  await vi.waitFor(() => expect(backend.commands.some(c => c.op === 'midi_off' && c.key === 60)).toBe(true));
  send({ kind: 'sustain', channel: 0, down: true });
  send({ kind: 'on', channel: 0, note: 62, velocity: 100 }); send({ kind: 'off', channel: 0, note: 62 });
  document.getElementById('stop')!.click();
  await vi.waitFor(() => expect(backend.commands.some(c => c.op === 'stop')).toBe(true));
  backend.commands.length = 0;
  send({ kind: 'on', channel: 0, note: 64, velocity: 100 }); send({ kind: 'off', channel: 0, note: 64 });
  await vi.waitFor(() => expect(backend.commands.some(c => c.op === 'midi_off' && c.key === 64)).toBe(true));
});


it('learns a hardware CC on an instrument slider, persists it, and sends its value to the engine', async () => {
  document.querySelector<HTMLButtonElement>('[data-view="instrument"]')!.click();
  const slider = document.querySelector<HTMLInputElement>('#instr-controls input[type=range]')!;
  expect(slider).not.toBeNull();
  const initial = slider.value;
  document.querySelector<HTMLButtonElement>('#midi-learn button')!.click();
  slider.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, cancelable: true }));
  backend.midi!({ kind: 'control', channel: 2, controller: 74, value: 127 });
  expect(slider.value).toBe(initial);
  backend.commands.length = 0;
  backend.midi!({ kind: 'control', channel: 2, controller: 74, value: 0 });
  await vi.waitFor(() => expect(backend.commands.some(c => c.op === 'update' && Object.values(c.song.instrument_values).some(v => (v as Record<string, number>).level === 0))).toBe(true));
  const saved = JSON.parse(localStorage.getItem('niminal-tracker-project-v12')!);
  expect(saved.tracks.some((t: any) => t.midi?.some((b: any) => b.channel === 2 && b.controller === 74))).toBe(true);
  expect(document.querySelector<HTMLInputElement>('#instr-controls input[type=range]')!.value).toBe('0');
});
