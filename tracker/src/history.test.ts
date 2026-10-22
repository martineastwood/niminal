import { expect, it } from 'vitest';
import { NoteHistory, parseNotes } from './history';
import { newProject } from './project';

it('undoes a multi-cell edit together, redoes it, and discards redo after a new edit', () => {
  const tracks = newProject().patterns[0].notes;
  const history = new NoteHistory();
  history.apply(tracks, [{ track: 0, row: 0, before: 'C-4', after: 'D-4' }, { track: 1, row: 0, before: null, after: 'E-4' }]);
  history.undo(tracks);
  expect(tracks[0][0]).toBe('C-4'); expect(tracks[1][0]).toBeNull();
  history.redo(tracks);
  expect(tracks[0][0]).toBe('D-4'); expect(tracks[1][0]).toBe('E-4');
  history.undo(tracks);
  history.apply(tracks, [{ track: 1, row: 0, before: null, after: 'G-4' }]);
  expect(history.redo(tracks)).toBe(false);
  expect(tracks[1][0]).toBe('G-4');
});
it('accepts rectangular note data and rejects malformed clipboard input', () => {
  expect(parseNotes('C-4\t---\r\nD#4\tG-3')).toEqual([['C-4', null], ['D#4', 'G-3']]);
  expect(parseNotes('C-4\tunknown')).toBeNull();
  expect(parseNotes(Array(258).fill('C-4').join('\n'))).toBeNull();
});
