import { expect, it } from 'vitest';
import { keyboardNote, newProject, readProject } from './project';

it('round trips source and notes and rejects unsupported project data', () => {
  const project = newProject();
  project.definitions[0].source += '// saved edit\n';
  project.tracks[2].notes[7] = 'F#3';
  expect(readProject(JSON.stringify(project))).toEqual(project);
  expect(() => readProject('{"version":1}')).toThrow();
  project.definitions.push(project.definitions[0]);
  expect(() => readProject(JSON.stringify(project))).toThrow();
});
it('maps the laptop keyboard chromatically without consuming navigation keys', () => {
  expect(keyboardNote('z', 4)).toBe('C-4');
  expect(keyboardNote('s', 4)).toBe('C#4');
  expect(keyboardNote('M', 3)).toBe('B-3');
  expect(keyboardNote('ArrowUp', 4)).toBeNull();
  expect(keyboardNote('', 4)).toBeNull();
  expect(keyboardNote('z', NaN)).toBeNull();
});

it('rejects invalid assignments and tempo before a project reaches playback', () => {
  const project = newProject();
  project.tracks[0].instrument = 'soften';
  expect(() => readProject(JSON.stringify(project))).toThrow();
  project.tracks[0].instrument = 'tone';
  project.bpm = 0;
  expect(() => readProject(JSON.stringify(project))).toThrow();
});
