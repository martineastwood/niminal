// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { Arrangement } from './arrangement';
import { newProject, makePattern } from './project';

vi.stubGlobal('Option', function (text: string, value: string) {
  const option = document.createElement('option'); option.textContent = text; option.value = value; return option;
});
afterEach(() => document.body.replaceChildren());

function setup() {
  const project = newProject(), parent = document.createElement('div'); document.body.append(parent);
  makePattern(project); project.sequence.push(project.patterns[1].id, project.patterns[0].id);
  const selected = vi.fn(), edited = vi.fn(), arrangement = new Arrangement(parent, () => project, selected, edited); arrangement.reset();
  const cell = (step: number, track: number) => parent.querySelectorAll<HTMLButtonElement>(`.sequence-entry[data-position="${step}"] .matrix-cell`)[track];
  return { project, parent, selected, edited, cell };
}

it('shows a cell for each track on each step, blank when the track plays its own pattern', () => {
  const { project, parent, cell } = setup();
  expect(parent.querySelectorAll('.sequence-entry')).toHaveLength(3);
  expect(parent.querySelectorAll('.sequence-entry[data-position="0"] .matrix-cell')).toHaveLength(project.tracks.length);
  expect(cell(1, 0).textContent).toBe(''); expect(cell(1, 0).classList.contains('own')).toBe(true);
});

it('mutes with shift-click and unmutes the same way, and tells the owner', () => {
  const { project, selected, edited, cell } = setup();
  cell(1, 2).dispatchEvent(new MouseEvent('click', { shiftKey: true, bubbles: true }));
  expect(project.matrix).toEqual([[], [null, null, { kind: 'mute' }]]); expect(edited).toHaveBeenCalledTimes(1); expect(selected).toHaveBeenCalledWith(false);
  expect(cell(1, 2).textContent).toBe('✕'); expect(cell(1, 2).classList.contains('mute')).toBe(true);
  cell(1, 2).dispatchEvent(new MouseEvent('click', { shiftKey: true, bubbles: true }));
  expect(project.matrix).toBeUndefined(); expect(cell(1, 2).textContent).toBe('');
});

it('opens a menu to redirect a track to another pattern, then back to its own, and closes on Escape', () => {
  const { project, cell } = setup();
  cell(0, 1).click();
  const menu = document.querySelector('.menu')!;
  expect([...menu.querySelectorAll('button')].map(b => b.textContent)).toEqual([`Own pattern · 00 ${project.patterns[0].name}`, 'Mute', `→ 01 ${project.patterns[1].name}`]);
  expect(menu.querySelector('[aria-checked=true]')!.textContent).toContain('Own pattern');
  menu.querySelectorAll('button')[2].click();
  expect(project.matrix).toEqual([[null, { kind: 'pattern', id: project.patterns[1].id }]]); expect(document.querySelector('.menu')).toBeNull();
  expect(cell(0, 1).textContent).toBe('01'); expect(cell(0, 1).classList.contains('pattern')).toBe(true);
  cell(0, 1).click(); expect(document.querySelector('.menu [aria-checked=true]')!.textContent).toContain('→ 01');
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); expect(document.querySelector('.menu')).toBeNull();
  cell(0, 1).click(); document.querySelectorAll<HTMLButtonElement>('.menu button')[0].click(); expect(project.matrix).toBeUndefined();
});
