// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { MatrixPanel } from './matrix';
import { newProject, makePattern } from './project';

vi.stubGlobal('Option', function (text: string, value: string) {
  const option = document.createElement('option'); option.textContent = text; option.value = value; return option;
});
afterEach(() => document.body.replaceChildren());

function setup() {
  const project = newProject(), parent = document.createElement('div'); document.body.append(parent);
  makePattern(project); project.sequence.push(project.patterns[1].id, project.patterns[0].id);
  const actions = { select: vi.fn(), edited: vi.fn() }, panel = new MatrixPanel(parent, () => project, actions); panel.render(0);
  const cell = (step: number, track: number) => parent.querySelectorAll<HTMLButtonElement>(`.matrix-row[data-step="${step}"] .matrix-cell`)[track];
  return { project, parent, actions, panel, cell };
}

it('shows a row for each step and a column for each track, each cell naming where the track plays from', () => {
  const { project, parent, cell } = setup();
  expect(parent.querySelectorAll('.matrix-row[data-step]')).toHaveLength(3); expect(parent.querySelectorAll('.matrix-track')).toHaveLength(project.tracks.length);
  expect(cell(0, 0).textContent).toBe('00'); expect(cell(1, 0).textContent).toBe('01'); expect(cell(2, 3).textContent).toBe('00');
  expect(parent.querySelector('.matrix-row[data-step="0"]')!.classList.contains('selected')).toBe(true);
});

it('mutes with shift-click and unmutes the same way, and tells the owner', () => {
  const { project, actions, cell } = setup();
  cell(1, 2).dispatchEvent(new MouseEvent('click', { shiftKey: true, bubbles: true }));
  expect(project.matrix).toEqual([[], [null, null, { kind: 'mute' }]]); expect(actions.edited).toHaveBeenCalledTimes(1);
  expect(cell(1, 2).textContent).toBe('✕'); expect(cell(1, 2).classList.contains('mute')).toBe(true);
  cell(1, 2).dispatchEvent(new MouseEvent('click', { shiftKey: true, bubbles: true }));
  expect(project.matrix).toBeUndefined(); expect(cell(1, 2).textContent).toBe('01');
});

it('opens a menu to redirect a track to another pattern, then back to its own, and closes on Escape', () => {
  const { project, cell } = setup();
  cell(0, 1).click();
  const menu = document.querySelector('.menu')!;
  expect([...menu.querySelectorAll('button')].map(b => b.textContent)).toEqual([`Own pattern · 00 ${project.patterns[0].name}`, 'Mute', `→ 01 ${project.patterns[1].name}`]);
  expect(menu.querySelector('[aria-checked=true]')!.textContent).toContain('Own pattern');
  menu.querySelectorAll('button')[2].click();
  expect(project.matrix).toEqual([[null, { kind: 'pattern', id: project.patterns[1].id }]]); expect(document.querySelector('.menu')).toBeNull();
  expect(cell(0, 1).textContent).toBe('→01'); expect(cell(0, 1).classList.contains('pattern')).toBe(true);
  cell(0, 1).click(); expect(document.querySelector('.menu [aria-checked=true]')!.textContent).toContain('→ 01');
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); expect(document.querySelector('.menu')).toBeNull();
  cell(0, 1).click(); document.querySelectorAll<HTMLButtonElement>('.menu button')[0].click(); expect(project.matrix).toBeUndefined();
});

it('selects a step from its label, highlights it, and marks the playing step', () => {
  const { parent, actions, panel } = setup();
  parent.querySelector<HTMLButtonElement>('.matrix-row[data-step="2"] .matrix-step')!.click(); expect(actions.select).toHaveBeenCalledWith(2);
  panel.highlight(1); expect(parent.querySelector('.matrix-row.selected')!.getAttribute('data-step')).toBe('1');
  panel.paint(2); expect(parent.querySelector('.matrix-row.playing')!.getAttribute('data-step')).toBe('2'); panel.paint(null); expect(parent.querySelector('.playing')).toBeNull();
});
