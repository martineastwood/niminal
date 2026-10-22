import { expect, it } from 'vitest';
import { keyboardNote, newProject, emptyFxLane, makePattern, removeTrack, restoreTrack, readProject, resizePattern } from './project';

it('round trips source and notes and rejects unsupported project data', () => {
  const project = newProject();
  project.definitions[0].source += '// saved edit\n';
  project.patterns[0].notes[2][7] = 'F#3';
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
  project.definitions.push({ id: 'soften', name: 'Filter', kind: 'fx', source: 'fx soften() { out = in }' });
  project.tracks[0].instrument = 'soften';
  expect(() => readProject(JSON.stringify(project))).toThrow();
  project.tracks[0].instrument = 'tone';
  project.bpm = 0;
  expect(() => readProject(JSON.stringify(project))).toThrow();
});

it('accepts more tracks only when every pattern has matching note columns', () => {
  const project = newProject();
  project.tracks.push({ ...project.tracks[0], name: 'Extra' });
  expect(() => readProject(JSON.stringify(project))).toThrow();
  project.patterns[0].notes.push(Array(32).fill(null)); project.patterns[0].fx.push([emptyFxLane(32)]);
  expect(readProject(JSON.stringify(project)).tracks).toHaveLength(5);
  while (project.tracks.length < 17) {
    project.tracks.push({ ...project.tracks[0] });
    project.patterns[0].notes.push(Array(32).fill(null)); project.patterns[0].fx.push([emptyFxLane(32)]);
  }
  expect(() => readProject(JSON.stringify(project))).toThrow();
});

it('deletes a track across all patterns and restores its notes and mixer settings', () => {
  const project = newProject();
  project.definitions.push({ id: 'soften', name: 'Filter', kind: 'fx', source: 'fx soften() { out = in }' });
  project.tracks[1].fx = { definition: 'soften', values: {}, modulations: {} }; project.tracks[1].volume_db = -7;
  project.patterns[0].notes[1][5] = 'F#3';
  const deleted = removeTrack(project, 1)!;
  expect(project.tracks).toHaveLength(3);
  expect(readProject(JSON.stringify(project))).toEqual(project);
  const added = makePattern(project);
  expect(restoreTrack(project, deleted)).toBe(true);
  expect(project.tracks[1].volume_db).toBe(-7);
  expect(project.patterns[0].notes[1][5]).toBe('F#3');
  expect(added.notes[1]).toEqual(Array(32).fill(null));
  expect(readProject(JSON.stringify(project))).toEqual(project);
  while (project.tracks.length > 1) removeTrack(project, 0);
  expect(removeTrack(project, 0)).toBeNull();
});

it('round trips named lanes, processors and LFO settings and rejects malformed values/modulation', () => {
  const project = newProject();
  project.definitions.push({ id: 'soften', name: 'Filter', kind: 'fx', source: 'fx soften() { out = in }' });
  project.tracks[0].fx = { definition: 'soften', values: { cutoff: 700 }, modulations: { cutoff: { enabled: true, shape: 'sine', period: 16, min: 0.1, max: 0.8 } } };
  project.patterns[0].fx[0][0].target = { kind: 'effect', parameter: 'cutoff' };
  project.patterns[0].fx[0][0].values[4] = '2400';
  expect(readProject(JSON.stringify(project))).toEqual(project);
  const copy = makePattern(project, project.patterns[0]); copy.fx[0][0].values[4] = '800';
  expect(project.patterns[0].fx[0][0].values[4]).toBe('2400');
  resizePattern(copy, 2); resizePattern(copy, 64); expect(copy.fx[0][0].values[4]).toBe('800');
  project.patterns[0].fx[0][0].values[4] = '0Q10'; expect(() => readProject(JSON.stringify(project))).toThrow();
  project.patterns[0].fx[0][0].values[4] = null;
  project.tracks[0].fx!.modulations.cutoff.period = 0; expect(() => readProject(JSON.stringify(project))).toThrow();
});

it('saves envelopes, deep-copies them into variations, retains hidden points and rejects invalid ordering', () => {
  const project = newProject(), lane = project.patterns[0].fx[0][0];
  lane.envelope = { enabled: true, mode: 'linear', points: [{ row: 0, value: -6 }, { row: 31.5, value: 0 }] };
  expect(readProject(JSON.stringify(project))).toEqual(project);
  const copy = makePattern(project, project.patterns[0]); copy.fx[0][0].envelope!.points[0].value = -12;
  expect(lane.envelope.points[0].value).toBe(-6);
  resizePattern(copy, 16); resizePattern(copy, 32); expect(copy.fx[0][0].envelope!.points[1].row).toBe(31.5);
  lane.envelope.points.reverse(); expect(() => readProject(JSON.stringify(project))).toThrow();
});
