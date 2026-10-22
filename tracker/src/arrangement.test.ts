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
