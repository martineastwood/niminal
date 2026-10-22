// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { renderMixer } from './mixer';
import { newProject } from './project';
import { copyBuiltInEffect } from './effects';

vi.stubGlobal('Option', function (text: string, value: string) {
  const option = document.createElement('option'); option.textContent = text; option.value = value; return option;
});
afterEach(() => document.body.replaceChildren());

it('inserts independent editable library copies into existing projects and exposes custom creation', () => {
  const project = newProject(), parent = document.createElement('div'), create = vi.fn(); document.body.append(parent);
  renderMixer(parent, project, 0, vi.fn(), vi.fn(), vi.fn(), {}, vi.fn(), false, create);
  const insert = () => parent.querySelector<HTMLButtonElement>('[aria-label="Add effect to chain"]')!.click();
  insert(); insert();
  const [a, b] = project.tracks[0].effects.map(slot => project.definitions.find(d => d.id === slot.definition)!);
  expect(a.name).toBe('Stereo delay'); expect(b.name).toBe('Stereo delay'); expect(a.id).not.toBe(b.id);
  expect(a.source).toContain(`fx ${a.id}(`); expect(b.source).toContain(`fx ${b.id}(`);
  a.source = 'custom edit'; expect(b.source).toContain('it.delay');
  parent.querySelector<HTMLButtonElement>('[aria-label="Create and insert custom effect"]')!.click(); expect(create).toHaveBeenCalledOnce();
});

it('uses the same editable controls and command recording for custom and built-in parameters', () => {
  const project = newProject(), parent = document.createElement('div'), edited = vi.fn(), write = vi.fn(); document.body.append(parent);
  const fx = copyBuiltInEffect(project, 'chorus'); project.tracks[0].effects.push({ definition: fx.id, values: {}, modulations: {} });
  renderMixer(parent, project, 0, vi.fn(), edited, vi.fn(), { [fx.id]: [{ name: 'mix', min: 0, max: 1, default: 0.4, unit: null }] }, write);
  const value = parent.querySelector<HTMLInputElement>('[aria-label="mix value"]')!;
  value.value = '0.8'; value.dispatchEvent(new Event('change'));
  expect(project.tracks[0].effects[0].values.mix).toBe(0.8); expect(edited).toHaveBeenCalledOnce();
  parent.querySelector<HTMLButtonElement>('[aria-label="Write mix to pattern"]')!.click(); expect(write).toHaveBeenCalledWith({ kind: 'effect', device: 0, parameter: 'mix' }, 0.8);
  value.value = '3'; value.dispatchEvent(new Event('change')); expect(project.tracks[0].effects[0].values.mix).toBe(1);
});
