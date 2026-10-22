// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { renderMixer } from './mixer';
import { newProject } from './project';
import { setTrackFxPreset } from './effects';

vi.stubGlobal('Option', function (text: string, value: string) {
  const option = document.createElement('option'); option.textContent = text; option.value = value; return option;
});
afterEach(() => document.body.replaceChildren());

it('replaces a track processor with editable presets while retaining named controls and automation', () => {
  const project = newProject();
  const a = setTrackFxPreset(project, 0, 'delay');
  const b = setTrackFxPreset(project, 1, 'delay');
  expect(a.id).not.toBe(b.id);
  project.tracks[0].fx!.values.mix = 0.8;
  const lane = project.patterns[0].fx[0][0]; lane.target = { kind: 'effect', parameter: 'mix' }; lane.values[4] = '0.5';
  const replacement = setTrackFxPreset(project, 0, 'chorus');
  expect(replacement.id).toBe(a.id); expect(replacement.source).toContain('in.chorus');
  expect(project.tracks[0].fx!.values.mix).toBe(0.8);
  expect(lane.target).toEqual({ kind: 'effect', parameter: 'mix' }); expect(lane.values[4]).toBe('0.5');
  expect(b.source).toContain('in.delay');
});

it('uses the same editable controls and command recording for custom and built-in parameters', () => {
  const project = newProject(), parent = document.createElement('div'), edited = vi.fn(), write = vi.fn(); document.body.append(parent);
  const fx = setTrackFxPreset(project, 0, 'chorus'); project.tracks[0].fx = { definition: fx.id, values: {}, modulations: {} };
  renderMixer(parent, project, 0, edited, { [fx.id]: [{ name: 'mix', min: 0, max: 1, default: 0.4, unit: null }] }, write);
  const value = parent.querySelector<HTMLInputElement>('[aria-label="mix value"]')!;
  value.value = '0.8'; value.dispatchEvent(new Event('change'));
  expect(project.tracks[0].fx!.values.mix).toBe(0.8); expect(edited).toHaveBeenCalledOnce();
  parent.querySelector<HTMLButtonElement>('[aria-label="Write mix to pattern"]')!.click(); expect(write).toHaveBeenCalledWith({ kind: 'effect', parameter: 'mix' }, 0.8);
  value.value = '3'; value.dispatchEvent(new Event('change')); expect(project.tracks[0].fx!.values.mix).toBe(1);
});
