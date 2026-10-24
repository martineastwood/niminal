import { expect, it } from 'vitest';
import { NoteHistory, ProjectHistory, parseNotes } from './history';
import { newProject } from './project';

it('undoes a multi-cell edit together, redoes it, and discards redo after a new edit', () => {
  const tracks = newProject().patterns[0].notes.map(columns => columns[0].notes);
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

it('restores instrument source, mixer, chains, structure and cells in chronological order', () => {
  const project = newProject(), history = new ProjectHistory(JSON.stringify(project));
  const snapshots = [JSON.stringify(project)];
  const edit = (change: () => void) => { change(); snapshots.push(JSON.stringify(project)); history.record(snapshots.at(-1)!); };
  edit(() => { project.definitions[0].source = 'rewritten instrument'; });
  edit(() => { project.tracks[0].volume_db = -12; });
  edit(() => { project.tracks[0].fx.push({ id: 'd1', definition: 'echo', bypass: false, values: {}, modulations: {} }); });
  edit(() => { project.patterns[0].notes[0][0].notes[0] = 'G-4'; });
  edit(() => { project.sequence.push(project.patterns[0].id); });
  for (let i = snapshots.length - 2; i >= 0; i--) expect(history.undo()).toBe(snapshots[i]);
  expect(history.undo()).toBeUndefined();
  for (let i = 1; i < snapshots.length; i++) expect(history.redo()).toBe(snapshots[i]);
  history.undo(); history.record('new edit'); expect(history.redo()).toBeUndefined();
  history.reset('other project'); expect(history.canUndo).toBe(false); expect(history.canRedo).toBe(false);
});

it('groups continuous source typing and slider gestures, with boundaries at other edits and undo', () => {
  const history = new ProjectHistory('original source');
  history.record('s', 'source:tone', 0); history.record('so', 'source:tone', 100);
  history.record('sou', 'source:tone', 200);
  history.record('mixer changed', 'gesture:1', 250); history.record('mixer changed more', 'gesture:1', 300);
  expect(history.undo()).toBe('sou'); expect(history.undo()).toBe('original source');
  expect(history.redo()).toBe('sou');
  history.record('new typing', 'source:tone', 350);
  expect(history.undo()).toBe('sou'); expect(history.canRedo).toBe(true);
});
