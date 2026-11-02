// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { MidiLearn, midiControl } from './midi-mapping';
import { newProject, readProject, duplicateTrack, moveTrack, removeTrack, restoreTrack, type MidiTarget, type Track, type EffectParameters } from './project';

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.replaceChildren(); vi.useRealTimers(); });
function setup() {
  const project = newProject(), parent = document.createElement('div'); document.body.append(parent);
  const changed = vi.fn(), record = vi.fn(), refresh = vi.fn(), feedback = vi.fn();
  const parameters: EffectParameters = { tone: [{ name: 'cutoff', min: 20, max: 20000, default: 1000, unit: 'hz' }] };
  const midi = new MidiLearn(parent, { project: () => project, parameters: () => parameters, changed, record, refresh, feedback });
  dispose = () => midi.dispose();
  const control = (strip: Track, target: MidiTarget) => {
    const input = document.createElement('input'); input.type = 'range'; input.value = '50'; document.body.append(input); midiControl(input, strip, target); return input;
  };
  const cc = (value: number, controller = 7, channel = 0) => midi.receive({ kind: 'control', channel, controller, value });
  const learn = (input: HTMLElement, controller = 7, channel = 0) => {
    parent.querySelector('button')!.click(); input.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, cancelable: true })); cc(100, controller, channel);
  };
  return { project, parent, midi, control, cc, learn, changed, record, refresh, feedback, parameters };
}

it('learns without a value jump, scales the full range, and isolates channels', () => {
  const { project, control, learn, cc, record } = setup(), strip = project.tracks[0];
  const before = strip.volume_db;
  learn(control(strip, { kind: 'volume' }), 7, 2);
  expect(strip.volume_db).toBe(before); expect(record).not.toHaveBeenCalled();
  cc(127, 7, 1); expect(strip.volume_db).toBe(before);
  cc(0, 7, 2); expect(strip.volume_db).toBe(-60);
  cc(127, 7, 2); expect(strip.volume_db).toBe(6);
  expect(record).toHaveBeenLastCalledWith(strip, { kind: 'volume' }, 6);
});

it('centres pan exactly and maps instrument and effect parameters in physical units', () => {
  const { project, parameters, control, learn, cc } = setup(), strip = project.tracks[0];
  learn(control(strip, { kind: 'pan' })); cc(64); expect(strip.pan).toBe(0);
  cc(0); expect(strip.pan).toBe(-1); cc(127); expect(strip.pan).toBe(1);
  const instrument = project.definitions.find(d => d.kind === 'instr')!;
  parameters[instrument.id] = parameters.tone;
  learn(control(strip, { kind: 'instrument', instrument: instrument.id, parameter: 'cutoff' }), 10);
  cc(0, 10); expect(instrument.values?.cutoff).toBe(20);
  cc(127, 10); expect(instrument.values?.cutoff).toBe(20000);
  project.definitions.push({ id: 'filter', name: 'Filter', kind: 'fx', source: 'fx filter { in }' });
  strip.fx.push({ id: 'filter1', definition: 'filter', values: {}, bypass: false, modulations: {} });
  parameters.filter = parameters.tone;
  learn(control(strip, { kind: 'effect', device: 'filter1', parameter: 'cutoff' }), 11);
  cc(127, 11); expect(strip.fx[0].values.cutoff).toBe(20000);
});

it('reassignment replaces the previous binding and mappings survive save and track moves', () => {
  const { project, control, learn, cc } = setup(), first = project.tracks[0], second = project.tracks[1];
  learn(control(first, { kind: 'volume' })); learn(control(second, { kind: 'pan' }));
  expect(first.midi).toBeUndefined(); expect(second.midi).toHaveLength(1);
  expect(readProject(JSON.stringify(project)).tracks[1].midi).toEqual(second.midi);
  moveTrack(project, 1, 0); cc(0); expect(project.tracks[0].pan).toBe(-1);
  const copy = duplicateTrack(project, 0)!; expect(project.tracks[copy].midi).toBeUndefined();
  const removed = removeTrack(project, 0)!;
  learn(control(first, { kind: 'volume' }));
  restoreTrack(project, removed); expect(removed.track.midi).toBeUndefined();
  expect(() => readProject(JSON.stringify(project))).not.toThrow();
});

it('does not drive missing or macro/LFO-owned effects and refuses invalid CC data', () => {
  const { project, parameters, control, learn, cc, feedback } = setup(), strip = project.tracks[0];
  strip.fx.push({ id: 'fx1', definition: 'filter', values: {}, bypass: false, modulations: {} }); parameters.filter = parameters.tone;
  learn(control(strip, { kind: 'effect', device: 'fx1', parameter: 'cutoff' }));
  strip.fx[0].modulations.cutoff = { enabled: true, shape: 'sine', period: 4, min: 0, max: 1 };
  cc(127); expect(strip.fx[0].values.cutoff).toBeUndefined(); expect(feedback).toHaveBeenLastCalledWith(expect.stringContaining('LFO or macro'));
  strip.fx.length = 0; expect(() => cc(127)).not.toThrow();
  const snapshot = JSON.stringify(project);
  cc(NaN); cc(128); cc(-1); cc(127, 64); cc(127, 7, 16);
  expect(JSON.stringify(project)).toBe(snapshot);
});

it('cancels learning with Escape and removes bindings through the mapping menu', () => {
  const { project, parent, control, learn, cc } = setup(), strip = project.tracks[0];
  learn(control(strip, { kind: 'volume' }));
  parent.querySelector('button')!.click();
  const escape = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }); window.dispatchEvent(escape);
  expect(escape.defaultPrevented).toBe(true); expect(parent.querySelector('button')!.getAttribute('aria-pressed')).toBe('false');
  parent.querySelectorAll('button')[1].click(); document.querySelector<HTMLButtonElement>('.menu button')!.click();
  expect(strip.midi).toBeUndefined(); const before = strip.volume_db; cc(0); expect(strip.volume_db).toBe(before);
});

it('throttles redraws while keeping every controller value and undo gesture', () => {
  vi.useFakeTimers();
  const { project, control, learn, cc, refresh, changed, record } = setup(), strip = project.tracks[0];
  learn(control(strip, { kind: 'volume' })); changed.mockClear();
  for (let value = 0; value < 128; value++) cc(value);
  expect(record).toHaveBeenCalledTimes(128); expect(refresh).not.toHaveBeenCalled();
  vi.advanceTimersByTime(50); expect(refresh).toHaveBeenCalledOnce();
  expect(changed).toHaveBeenLastCalledWith('midi:0:7'); expect(strip.volume_db).toBe(6);
});
