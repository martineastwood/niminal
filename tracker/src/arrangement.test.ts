// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { Arrangement } from './arrangement';
import { newProject, readProject, resizePattern } from './project';

vi.stubGlobal('Option', function (text: string, value: string) {
  const option = document.createElement('option'); option.textContent = text; option.value = value; return option;
});
afterEach(() => document.body.replaceChildren());

it('reuses pattern references and duplicates variations without copying song-wide track settings', () => {
  const project = newProject(), parent = document.createElement('div');
  const view = new Arrangement(parent, () => project, vi.fn(), vi.fn()); view.reset();
  const original = view.pattern;
  view.insert('reuse');
  expect(project.sequence).toEqual([original.id, original.id]);
  view.pattern.notes[0][0].notes[1] = 'F#4';
  expect(project.patterns[0].notes[0][0].notes[1]).toBe('F#4');
  view.insert('variation');
  const variation = view.pattern;
  expect(variation.id).not.toBe(original.id);
  expect(variation.notes).toEqual(original.notes);
  variation.notes[0][0].notes[1] = 'G-4';
  expect(original.notes[0][0].notes[1]).toBe('F#4');
  expect(project.tracks).toHaveLength(4);
  expect(readProject(JSON.stringify(project))).toEqual(project);
});

it('reorders and removes sequence entries while keeping patterns available for reuse', () => {
  const project = newProject(), parent = document.createElement('div');
  const view = new Arrangement(parent, () => project, vi.fn(), vi.fn()); view.reset();
  const first = view.pattern;
  view.insert('new'); const second = view.pattern;
  view.move(-1); expect(project.sequence).toEqual([second.id, first.id]);
  view.remove(); expect(project.sequence).toEqual([first.id]);
  expect(project.patterns).toHaveLength(2);
  view.remove(); expect(project.sequence).toHaveLength(1);
  view.choose(second.id); view.insert('reuse');
  expect(project.sequence).toEqual([first.id, second.id]);
  view.paint(1, true);
  expect(parent.querySelectorAll('.playing')).toHaveLength(1);
  expect([...parent.querySelectorAll('select')].every(select => select.disabled)).toBe(true);
});

it('retains notes when shortening a pattern and rejects missing sequence references', () => {
  const project = newProject(), pattern = project.patterns[0];
  pattern.notes[0][0].notes[31] = 'B-4';
  resizePattern(pattern, 8); expect(pattern.rows).toBe(8);
  resizePattern(pattern, 64); expect(pattern.notes[0][0].notes[31]).toBe('B-4');
  expect(pattern.notes.every(track => track.every(column => column.notes.length === 64 && column.vol.length === 64))).toBe(true);
  expect(readProject(JSON.stringify(project))).toEqual(project);
  project.sequence.push('missing');
  expect(() => readProject(JSON.stringify(project))).toThrow();
});

it('loops the selected step or a shift-clicked range, and keeps the loop on the same steps as the sequence changes', () => {
  const project = newProject(), parent = document.createElement('div'), edited = vi.fn();
  const view = new Arrangement(parent, () => project, vi.fn(), edited); view.reset();
  for (let i = 0; i < 4; i++) view.insert('reuse'); // five steps
  view.select(1);
  view.toggleLoop(); expect(project.loop_range).toEqual({ start: 1, end: 1 });
  expect(parent.querySelectorAll('.in-loop')).toHaveLength(1);
  parent.querySelector<HTMLButtonElement>('.sequence-entry[data-position="3"] button')!.dispatchEvent(new MouseEvent('click', { shiftKey: true, bubbles: true }));
  expect(project.loop_range).toEqual({ start: 1, end: 3 }); expect(edited).toHaveBeenCalled();
  expect([...parent.querySelectorAll('.in-loop')].map(row => (row as HTMLElement).dataset.position)).toEqual(['1', '2', '3']);
  expect(parent.querySelector('.loop-start')!.getAttribute('data-position')).toBe('1'); expect(parent.querySelector('.loop-end')!.getAttribute('data-position')).toBe('3');
  expect(readProject(JSON.stringify(project)).loop_range).toEqual({ start: 1, end: 3 });
  // A step added before the loop pushes it along, inside it joins it, and after it leaves it.
  view.select(0); view.insert('reuse'); expect(project.loop_range).toEqual({ start: 2, end: 4 });
  view.select(2); view.insert('reuse'); expect(project.loop_range).toEqual({ start: 2, end: 5 });
  view.select(5); view.insert('reuse'); expect(project.loop_range).toEqual({ start: 2, end: 5 });
  // Removing steps shifts it, shrinks it, and clears it once it is empty.
  view.select(0); view.remove(); expect(project.loop_range).toEqual({ start: 1, end: 4 });
  view.select(2); view.remove(); expect(project.loop_range).toEqual({ start: 1, end: 3 });
  view.select(5); view.remove(); expect(project.loop_range).toEqual({ start: 1, end: 3 });
  view.select(1); view.toggleLoop(); expect(project.loop_range).toBeUndefined();
  view.toggleLoop(); view.remove(); expect(project.loop_range).toBeUndefined();
});

it('rejects a loop outside the sequence', () => {
  const project = newProject();
  for (const loop_range of [{ start: 1, end: 1 }, { start: 0, end: 1 }, { start: 1, end: 0 }, { start: 0.5, end: 0 }]) expect(() => readProject(JSON.stringify({ ...project, loop_range }))).toThrow();
  expect(readProject(JSON.stringify({ ...project, loop_range: { start: 0, end: 0 } })).loop_range).toEqual({ start: 0, end: 0 });
});

it('groups steps into named sections that collapse, loop, and follow the sequence as it changes', () => {
  const project = newProject(), parent = document.createElement('div'), edited = vi.fn();
  const view = new Arrangement(parent, () => project, vi.fn(), edited); view.reset();
  for (let i = 0; i < 5; i++) view.insert('reuse'); // six steps
  const steps = () => parent.querySelectorAll('.sequence-entry').length, headers = () => [...parent.querySelectorAll<HTMLInputElement>('.sequence-section input')].map(i => i.value);
  view.select(0); view.toggleSection(); view.select(2); view.toggleSection(); view.select(4); view.toggleSection();
  expect(project.sections).toEqual([{ start: 0, name: 'Section 1' }, { start: 2, name: 'Section 2' }, { start: 4, name: 'Section 3' }]);
  expect(headers()).toEqual(['Section 1', 'Section 2', 'Section 3']); expect(steps()).toBe(6);
  expect(parent.querySelector('.sequence-section[data-start="2"] .sequence-count')!.textContent).toBe('2');
  expect(readProject(JSON.stringify(project)).sections).toEqual(project.sections);
  // Renaming and collapsing.
  const name = parent.querySelector<HTMLInputElement>('.sequence-section[data-start="2"] input')!; name.value = '  Chorus '; name.dispatchEvent(new Event('change'));
  expect(project.sections![1].name).toBe('Chorus');
  parent.querySelector<HTMLButtonElement>('[aria-label="Collapse Chorus"]')!.click();
  expect(project.sections![1].collapsed).toBe(true); expect(steps()).toBe(4);
  view.paint(3, true); expect(parent.querySelector('.sequence-section[data-start="2"]')!.classList.contains('playing')).toBe(true);
  parent.querySelector<HTMLButtonElement>('[aria-label="Expand Chorus"]')!.click(); expect(project.sections![1].collapsed).toBeUndefined(); expect(steps()).toBe(6);
  // Loop a section; again clears it.
  parent.querySelector<HTMLButtonElement>('[aria-label="Loop Chorus"]')!.click(); expect(project.loop_range).toEqual({ start: 2, end: 3 });
  parent.querySelector<HTMLButtonElement>('[aria-label="Loop Chorus"]')!.click(); expect(project.loop_range).toBeUndefined();
  parent.querySelector<HTMLButtonElement>('[aria-label="Loop Section 3"]')!.click(); expect(project.loop_range).toEqual({ start: 4, end: 5 });
  // A step added inside a section joins it, one added after its last step joins the section before the next one.
  view.select(2); view.insert('reuse'); expect(project.sections!.map(s => s.start)).toEqual([0, 2, 5]);
  view.select(0); view.insert('reuse'); expect(project.sections!.map(s => s.start)).toEqual([0, 3, 6]);
  // Removing steps moves later sections up, keeps a section that still has steps, and drops one left empty.
  view.select(1); view.remove(); expect(project.sections!.map(s => s.start)).toEqual([0, 2, 5]);
  view.select(2); view.remove(); expect(project.sections!.map(s => s.start)).toEqual([0, 2, 4]);
  view.select(2); view.remove(); expect(project.sections!.map(s => s.start)).toEqual([0, 2, 3]);
  view.select(2); view.remove(); expect(project.sections!.map(s => s.name)).toEqual(['Section 1', 'Section 3']); expect(project.sections!.map(s => s.start)).toEqual([0, 2]);
  // Marking the same step again removes the section; with none left the field goes.
  view.select(2); view.toggleSection(); view.select(0); view.toggleSection(); expect(project.sections).toBeUndefined();
});

it('rejects sections that are out of order or outside the sequence', () => {
  const project = newProject(); for (let i = 0; i < 3; i++) project.sequence.push(project.patterns[0].id);
  const read = (sections: unknown) => () => readProject(JSON.stringify({ ...project, sections }));
  expect(read([{ start: 0, name: 'A' }, { start: 2, name: 'B', collapsed: true }])).not.toThrow();
  for (const bad of [[{ start: 4, name: 'A' }], [{ start: 2, name: 'A' }, { start: 2, name: 'B' }], [{ start: 2, name: 'A' }, { start: 1, name: 'B' }], [{ start: 0, name: 7 }], [{ start: 0, name: 'A', collapsed: 'yes' }], 'x']) expect(read(bad)).toThrow();
});
