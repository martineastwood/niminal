// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { renderMixer, renderInstrument, type FxActions } from './mixer';
import { newProject } from './project';
import { addDevice, moveDevice, removeDevice } from './effects';

vi.stubGlobal('Option', function (text: string, value: string) {
  const option = document.createElement('option'); option.textContent = text; option.value = value; return option;
});
afterEach(() => document.body.replaceChildren());

function actions(overrides: Partial<FxActions> = {}): FxActions {
  return { selected: null, select: vi.fn(), add: vi.fn(), remove: vi.fn(), move: vi.fn(), changed: vi.fn(), ...overrides };
}

it('builds a chain from presets, each device with its own copy of the code', () => {
  const project = newProject();
  const a = addDevice(project, 0, 'delay')!, b = addDevice(project, 0, 'delay')!, c = addDevice(project, 1, 'chorus')!;
  expect(a.definition.id).not.toBe(b.definition.id);
  expect(project.tracks[0].fx.map(d => d.id)).toEqual([a.device.id, b.device.id]);
  expect([a.definition.name, b.definition.name]).toEqual(['Stereo delay', 'Stereo delay 2']);
  expect(a.definition.source).toContain(`fx ${a.definition.id}`); expect(a.definition.source).toContain('in.delay');
  expect(c.definition.source).toContain('in.chorus');
  expect(moveDevice(project, 0, b.device.id, -1)).toBe(true);
  expect(project.tracks[0].fx.map(d => d.id)).toEqual([b.device.id, a.device.id]);
  expect(moveDevice(project, 0, b.device.id, -1)).toBe(false);
  const lane = project.patterns[0].fx[0][0]; lane.target = { kind: 'effect', device: a.device.id, parameter: 'mix' };
  const kept = project.patterns[0].fx[0][0];
  expect(removeDevice(project, 0, a.device.id)!.id).toBe(a.definition.id);
  expect(project.definitions.some(d => d.id === a.definition.id)).toBe(false);
  expect(kept.target).toBeNull();
  for (let i = 0; i < 10; i++) addDevice(project, 2, 'notch');
  expect(project.tracks[2].fx).toHaveLength(8);
});

it('keeps lanes that hold values when their effect is removed', () => {
  const project = newProject(); const { device } = addDevice(project, 0, 'delay')!;
  const lane = project.patterns[0].fx[0][0]; lane.target = { kind: 'effect', device: device.id, parameter: 'mix' }; lane.values[4] = '0.5';
  removeDevice(project, 0, device.id);
  expect(lane.target).toEqual({ kind: 'effect', device: device.id, parameter: 'mix' });
});

it('uses the same editable controls and command recording for custom and built-in parameters', () => {
  const project = newProject(), parent = document.createElement('div'), edited = vi.fn(), write = vi.fn(); document.body.append(parent);
  const { device, definition } = addDevice(project, 0, 'chorus')!;
  renderMixer(parent, project, 0, edited, { [definition.id]: [{ name: 'mix', min: 0, max: 1, default: 0.4, unit: null }] }, write, actions({ selected: device.id }));
  const value = parent.querySelector<HTMLInputElement>('[aria-label="mix value"]')!;
  value.value = '0.8'; value.dispatchEvent(new Event('change'));
  expect(device.values.mix).toBe(0.8); expect(edited).toHaveBeenCalledOnce();
  parent.querySelector<HTMLButtonElement>('[aria-label="Write mix to pattern"]')!.click(); expect(write).toHaveBeenCalledWith({ kind: 'effect', device: device.id, parameter: 'mix' }, 0.8);
  value.value = '3'; value.dispatchEvent(new Event('change')); expect(device.values.mix).toBe(1);
});

it('shows each device as a card whose buttons act on the chain, with only the selected one expanded', () => {
  const project = newProject(), parent = document.createElement('div'); document.body.append(parent);
  const a = addDevice(project, 0, 'delay')!, b = addDevice(project, 0, 'lowpass')!;
  const fx = actions({ selected: b.device.id });
  renderMixer(parent, project, 0, vi.fn(), { [a.definition.id]: [{ name: 'mix', min: 0, max: 1, default: 0.3, unit: null }], [b.definition.id]: [{ name: 'cutoff', min: 20, max: 20000, default: 2400, unit: 'hz' }] }, vi.fn(), fx);
  expect(parent.querySelectorAll('.device')).toHaveLength(2);
  expect(parent.querySelector('[aria-label="mix value"]')).toBeNull(); expect(parent.querySelector('[aria-label="cutoff value"]')).not.toBeNull();
  parent.querySelector<HTMLButtonElement>('[aria-label="Edit Stereo delay"]')!.click(); expect(fx.select).toHaveBeenCalledWith(a.device.id);
  parent.querySelector<HTMLButtonElement>('[aria-label="Bypass Stereo delay"]')!.click(); expect(a.device.bypass).toBe(true); expect(fx.changed).toHaveBeenCalled();
  parent.querySelector<HTMLButtonElement>('[aria-label="Move effect later"]')!.click(); expect(fx.move).toHaveBeenCalledWith(a.device.id, 1);
  parent.querySelector<HTMLButtonElement>('[aria-label="Remove Low pass"]')!.click(); expect(fx.remove).toHaveBeenCalledWith(b.device.id);
  expect(parent.querySelector<HTMLButtonElement>('[aria-label="Move effect earlier"]')!.disabled).toBe(true);
  const add = parent.querySelector<HTMLSelectElement>('[aria-label="Add effect"]')!; add.value = 'notch'; add.dispatchEvent(new Event('change')); expect(fx.add).toHaveBeenCalledWith('notch');
});

it('shows an instrument\'s parameters as sliders that set its values, and records them to a lane only for the track that plays it', () => {
  const project = newProject(), parent = document.createElement('div'), edited = vi.fn(), write = vi.fn(); document.body.append(parent);
  const tone = project.definitions[0];
  const parameters = { tone: [{ name: 'cutoff', min: 200, max: 8000, default: 2000, unit: 'hz' }] };
  renderInstrument(parent, project, tone, {}, edited, write, 0); expect(parent.textContent).toContain('Apply to expose parameters');
  renderInstrument(parent, project, tone, parameters, edited, write, 0);
  const value = parent.querySelector<HTMLInputElement>('[aria-label="cutoff value"]')!;
  expect(value.value).toBe('2000'); value.value = '500'; value.dispatchEvent(new Event('change'));
  expect(tone.values).toEqual({ cutoff: 500 }); expect(edited).toHaveBeenCalledOnce();
  parent.querySelector<HTMLButtonElement>('[aria-label="Write cutoff to pattern"]')!.click();
  expect(write).toHaveBeenCalledWith({ kind: 'instrument', parameter: 'cutoff' }, 500);
  renderInstrument(parent, project, tone, parameters, edited, write, 1);
  expect(parent.querySelector<HTMLButtonElement>('[aria-label="Write cutoff to pattern"]')!.disabled).toBe(true);
});
