import { expect, it } from 'vitest';
import { keyboardNote, newProject, emptyFxLane, blankColumn, makePattern, removeTrack, restoreTrack, readProject, resizePattern } from './project';

it('round trips source and notes and rejects unsupported project data', () => {
  const project = newProject();
  project.definitions[0].source += '// saved edit\n';
  project.patterns[0].notes[2][0].notes[7] = 'F#3';
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
  project.patterns[0].notes.push([blankColumn(32)]); project.patterns[0].fx.push([emptyFxLane(32)]);
  expect(readProject(JSON.stringify(project)).tracks).toHaveLength(5);
  while (project.tracks.length < 17) {
    project.tracks.push({ ...project.tracks[0] });
    project.patterns[0].notes.push([blankColumn(32)]); project.patterns[0].fx.push([emptyFxLane(32)]);
  }
  expect(() => readProject(JSON.stringify(project))).toThrow();
});

it('deletes a track across all patterns and restores its notes and mixer settings', () => {
  const project = newProject();
  project.definitions.push({ id: 'soften', name: 'Filter', kind: 'fx', source: 'fx soften() { out = in }' });
  project.tracks[1].fx = [{ id: 'd1', definition: 'soften', bypass: false, values: {}, modulations: {} }]; project.tracks[1].volume_db = -7;
  project.patterns[0].notes[1][0].notes[5] = 'F#3';
  const deleted = removeTrack(project, 1)!;
  expect(project.tracks).toHaveLength(3);
  expect(readProject(JSON.stringify(project))).toEqual(project);
  const added = makePattern(project);
  expect(restoreTrack(project, deleted)).toBe(true);
  expect(project.tracks[1].volume_db).toBe(-7);
  expect(project.patterns[0].notes[1][0].notes[5]).toBe('F#3');
  expect(added.notes[1][0].notes).toEqual(Array(32).fill(null));
  expect(readProject(JSON.stringify(project))).toEqual(project);
  while (project.tracks.length > 1) removeTrack(project, 0);
  expect(removeTrack(project, 0)).toBeNull();
});

it('round trips named lanes, processors and LFO settings and rejects malformed values/modulation', () => {
  const project = newProject();
  project.definitions.push({ id: 'soften', name: 'Filter', kind: 'fx', source: 'fx soften() { out = in }' });
  project.tracks[0].fx = [{ id: 'd1', definition: 'soften', bypass: false, values: { cutoff: 700 }, modulations: { cutoff: { enabled: true, shape: 'sine', period: 16, min: 0.1, max: 0.8 } } }];
  project.patterns[0].fx[0][0].target = { kind: 'effect', device: 'd1', parameter: 'cutoff' };
  project.patterns[0].fx[0][0].values[4] = '2400';
  expect(readProject(JSON.stringify(project))).toEqual(project);
  const copy = makePattern(project, project.patterns[0]); copy.fx[0][0].values[4] = '800';
  expect(project.patterns[0].fx[0][0].values[4]).toBe('2400');
  resizePattern(copy, 2); resizePattern(copy, 64); expect(copy.fx[0][0].values[4]).toBe('800');
  project.patterns[0].fx[0][0].values[4] = '0Q10'; expect(() => readProject(JSON.stringify(project))).toThrow();
  project.patterns[0].fx[0][0].values[4] = null;
  project.tracks[0].fx[0].modulations.cutoff.period = 0; expect(() => readProject(JSON.stringify(project))).toThrow();
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

it('round trips note columns with OFF and sub-column values and rejects out-of-range cells', () => {
  const project = newProject(), column = project.patterns[0].notes[0][0];
  column.notes[2] = 'OFF'; column.vol[0] = '128'; column.pan[1] = '0'; column.delay[3] = '255'; column.inst[4] = 'tone';
  project.patterns[0].notes[1].push(blankColumn(32)); project.tracks[1].show = { vol: true };
  expect(readProject(JSON.stringify(project))).toEqual(project);
  const bad = (change: (c: typeof column) => void) => { const copy = structuredClone(project); change(copy.patterns[0].notes[0][0]); expect(() => readProject(JSON.stringify(copy))).toThrow(); };
  bad(c => { c.vol[0] = '129'; }); bad(c => { c.delay[0] = '256'; }); bad(c => { c.notes[0] = 'H-9'; });
  bad(c => { c.inst[0] = 'missing'; }); bad(c => { c.vol = Array(10).fill(null); }); bad(c => { c.pan[0] = '-1'; });
  const copy = makePattern(project, project.patterns[0]); copy.notes[0][0].vol[0] = '1';
  expect(project.patterns[0].notes[0][0].vol[0]).toBe('128'); expect(copy.notes[1]).toHaveLength(2);
  resizePattern(copy, 64); expect(copy.notes[0][0].delay).toHaveLength(64);
  const deleted = removeTrack(project, 1)!; expect(restoreTrack(project, deleted)).toBe(true); expect(project.patterns[0].notes[1]).toHaveLength(2);
});

it('keeps instrument parameter values and lanes, and rejects non-numeric values', () => {
  const project = newProject(); project.definitions[0].values = { cutoff: 900 };
  project.patterns[0].fx[0][0].target = { kind: 'instrument', parameter: 'cutoff' };
  expect(readProject(JSON.stringify(project))).toEqual(project);
  project.definitions[0].values = { cutoff: 'loud' as unknown as number };
  expect(() => readProject(JSON.stringify(project))).toThrow();
});

it('gives a new blank pattern as many note columns per track as the song already has', () => {
  const project = newProject(); project.patterns[0].notes[1].push(blankColumn(32));
  const fresh = makePattern(project);
  expect(fresh.notes.map(columns => columns.length)).toEqual([1, 2, 1, 1]); expect(fresh.notes[1][1].vol).toHaveLength(32);
});

it('keeps a master strip and solo flags, and rejects a master with an instrument or a bad solo flag', () => {
  const project = newProject();
  project.master.volume_db = -3; project.master.fx = [{ id: 'd1', definition: 'soften', bypass: false, values: {}, modulations: {} }];
  project.definitions.push({ id: 'soften', name: 'Filter', kind: 'fx', source: 'fx soften() { out = in }' });
  project.tracks[2].solo = true;
  expect(readProject(JSON.stringify(project))).toEqual(project);
  const bad = (change: (p: typeof project) => void) => { const copy = structuredClone(project); change(copy); expect(() => readProject(JSON.stringify(copy))).toThrow(); };
  bad(p => { p.master.instrument = 'tone'; }); bad(p => { p.master.volume_db = 20; }); bad(p => { (p.tracks[0] as { solo: unknown }).solo = 'yes'; });
  bad(p => { p.master.fx[0].definition = 'missing'; }); bad(p => { delete (p as { master?: unknown }).master; });
});
