// @vitest-environment happy-dom
import { afterAll, expect, it, vi } from 'vitest';
import type { AudioStatus } from './audio';

const backend = vi.hoisted(() => ({
  status: {
    connected: false, playing: false, row: null, row_phase: null, row_at_ms: 0,
    pattern_id: null, sequence_position: null, sample_rate: 48000,
    skipped_rows: 0, late_events: 0, error: null,
    instruments: {}, effects: {}, instrument_parameters: {}, effect_parameters: {},
  } as AudioStatus,
  commands: [] as Record<string, any>[],
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
vi.mock('./midi', () => ({ mountMidi: async () => () => {} }));
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
  const name = document.getElementById('pattern-name')!; name.focus();
  const before = record();
  name.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
  expect(record()).not.toBe(before);
  await vi.waitFor(() => expect(active()).toBe(grid));
  // a toolbar button leaves it there too
  const undo = document.getElementById('undo') as HTMLButtonElement; undo.focus(); undo.click();
  await vi.waitFor(() => expect(active()).toBe(grid));
  // a button of the sequence bar
  const first = document.querySelector<HTMLButtonElement>('#sequence .sequence-entry > button')!; first.focus(); first.click();
  await vi.waitFor(() => expect(active()).toBe(grid));
  // a choice made in a menu of the toolbar
  const picker = document.getElementById('pattern-picker') as HTMLSelectElement; picker.focus();
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
  const picker = document.getElementById('pattern-picker') as HTMLSelectElement; picker.focus();
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

it('applies an effect as soon as it is added to a track, so its controls appear without pressing Apply', async () => {
  backend.commands.length = 0;
  (document.querySelector('[aria-label="Add effect"]') as HTMLButtonElement).click();
  const item = [...document.querySelectorAll<HTMLElement>('button, [role=option], li')].find(e => e.textContent?.trim().startsWith('Echo'))!;
  item.click();
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
  (document.querySelector('[aria-label="Add effect"]') as HTMLButtonElement).click();
  [...document.querySelectorAll<HTMLElement>('button, [role=option], li')].find(e => e.textContent?.trim().startsWith('Lowpass'))!.click();
  expect(backend.commands.some(c => c.op === 'apply' && c.kind === 'fx')).toBe(false);
  backend.connect!();
  await vi.waitFor(() => expect(backend.commands.filter(c => c.op === 'apply' && c.kind === 'fx').length).toBeGreaterThan(0));
  await vi.waitFor(() => expect(document.getElementById('fx-status')!.textContent).toBe('Applied'));
  expect(document.getElementById('mixer')!.textContent).toContain('mix');
});
