// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { MixPanel } from './mix';
import { drawScope, meterLevel } from './scope';
import { newProject } from './project';

vi.stubGlobal('Option', function (text: string, value: string) {
  const option = document.createElement('option'); option.textContent = text; option.value = value; return option;
});

afterEach(() => document.body.replaceChildren());

function setup() {
  const parent = document.createElement('div'); document.body.append(parent);
  const project = newProject(), actions = { selectTrack: vi.fn(), openFx: vi.fn(), edited: vi.fn(), addReturn: vi.fn(), removeReturn: vi.fn() };
  const panel = new MixPanel(parent, () => project, actions); panel.render();
  return { parent, project, actions, panel };
}

it('shows a strip for every track and the master, and each strip acts on its own track', () => {
  const { parent, project, actions } = setup();
  expect(parent.querySelectorAll('.mix-strip')).toHaveLength(5); expect(parent.querySelector('.mix-strip.master')).not.toBeNull();
  parent.querySelector<HTMLButtonElement>('[aria-label="Mute Bass"]')!.click();
  expect(project.tracks[1].muted).toBe(true); expect(actions.edited).toHaveBeenCalled();
  expect(parent.querySelector('[aria-label="Mute Bass"]')!.getAttribute('aria-pressed')).toBe('true');
  parent.querySelector<HTMLButtonElement>('[aria-label="Solo Drums"]')!.click(); expect(project.tracks[2].solo).toBe(true);
  parent.querySelector<HTMLButtonElement>('[aria-label="Bass effect chain"]')!.click(); expect(actions.openFx).toHaveBeenCalledWith(1);
  parent.querySelectorAll<HTMLButtonElement>('.mix-name')[3].click(); expect(actions.selectTrack).toHaveBeenCalledWith(3);
  parent.querySelector<HTMLButtonElement>('.mix-strip.master .mix-name')!.click(); expect(actions.openFx).toHaveBeenCalledWith(-1);
  expect(parent.querySelector('.mix-strip.master [aria-label^="Mute"]')).toBeNull();
});

it('shows returns beside the master, with an output choice on every track', () => {
  const { parent, project, actions, panel } = setup();
  expect(parent.querySelector('.mix-output')!.children).toHaveLength(2);
  parent.querySelector<HTMLButtonElement>('.mix-add-return')!.click(); expect(actions.addReturn).toHaveBeenCalled();
  project.returns.push({ name: 'Return A', fx: [], pan: 0, muted: false, volume_db: 0 }, { name: 'Return B', fx: [], pan: 0, muted: false, volume_db: 0 });
  panel.render();
  expect(parent.querySelectorAll('.mix-strip')).toHaveLength(7);
  const output = parent.querySelector<HTMLSelectElement>('[aria-label="Bass output"]')!; expect([...output.options].map(o => o.text)).toEqual(['Master', 'Return A', 'Return B', 'None']);
  output.value = 'return1'; output.dispatchEvent(new Event('change'));
  expect(project.tracks[1].output).toEqual({ kind: 'return', index: 1 }); expect(actions.edited).toHaveBeenCalled();
  output.value = 'none'; output.dispatchEvent(new Event('change')); expect(project.tracks[1].output).toEqual({ kind: 'none' });
  // A return can only feed a later one.
  expect([...parent.querySelector<HTMLSelectElement>('[aria-label="Return A output"]')!.options].map(o => o.text)).toEqual(['Master', 'Return B', 'None']);
  expect([...parent.querySelector<HTMLSelectElement>('[aria-label="Return B output"]')!.options].map(o => o.text)).toEqual(['Master', 'None']);
  expect(parent.querySelector('.mix-strip.master .mix-output')).toBeNull();
  expect(parent.querySelector('.mix-strip.return [aria-label^="Solo"]')).toBeNull();
  parent.querySelector<HTMLButtonElement>('[aria-label="Return B effect chain"]')!.click(); expect(actions.openFx).toHaveBeenCalledWith(-3);
  parent.querySelector<HTMLButtonElement>('[aria-label="Remove Return A"]')!.click(); expect(actions.removeReturn).toHaveBeenCalledWith(0);
  panel.paint({ s_r1: [0.5, -0.5] }); expect(parseFloat(parent.querySelectorAll<HTMLElement>('.meter-fill')[5].style.height)).toBeGreaterThan(0);
});

it('sets level and pan from the faders, with the master as its own strip', () => {
  const { parent, project, actions } = setup();
  const lead = parent.querySelector<HTMLElement>('[aria-label="Lead volume in dB"]')!;
  lead.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
  expect(project.tracks[0].volume_db).toBe(-0.5); expect(parent.querySelector('.mix-strip .mix-db')!.textContent).toBe('-0.5');
  lead.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', shiftKey: true, bubbles: true })); expect(project.tracks[0].volume_db).toBe(-0.4);
  lead.dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); expect(project.tracks[0].volume_db).toBe(0);
  const master = parent.querySelector<HTMLElement>('[aria-label="Master volume in dB"]')!;
  for (let i = 0; i < 200; i++) master.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
  expect(project.master.volume_db).toBe(-60); expect(parent.querySelector('.mix-strip.master .mix-db')!.textContent).toBe('-∞');
  const pan = parent.querySelector<HTMLInputElement>('[aria-label="Lead pan"]')!; pan.value = '-0.5'; pan.dispatchEvent(new Event('input'));
  expect(project.tracks[0].pan).toBe(-0.5); expect(actions.edited).toHaveBeenCalled();
});

it('paints meters from the scopes and highlights the selected strip', () => {
  const { parent, panel } = setup();
  panel.paint({ s_0: [0, 0.5, -0.5, 0], s_m: [0.1, -0.1] });
  const level = (index: number) => parent.querySelectorAll<HTMLElement>('.meter-fill')[index].style.height;
  expect(level(0)).toMatch(/^\d/); expect(parseFloat(level(0))).toBeGreaterThan(parseFloat(level(1)));
  expect(parseFloat(level(4))).toBeGreaterThan(0);
  panel.highlight(2); expect(parent.querySelector('.mix-strip.selected')!.getAttribute('data-track')).toBe('2');
  panel.highlight(-1); expect(parent.querySelector('.mix-strip.selected')!.classList.contains('master')).toBe(true);
});

it('reads levels on a decibel scale and tolerates a missing scope', () => {
  expect(meterLevel(undefined)).toBe(0); expect(meterLevel([0, 0])).toBe(0);
  expect(meterLevel([1])).toBeCloseTo(60 / 66); expect(meterLevel([0.001])).toBeCloseTo(0);
  expect(() => drawScope(document.createElement('canvas'), [0.1, -0.1])).not.toThrow(); expect(() => drawScope(document.createElement('canvas'), undefined)).not.toThrow();
});
