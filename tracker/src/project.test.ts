import { patternSeconds, patternRow } from './project';
import { expect, it } from 'vitest';
import { emptyProject, keyboardNote, transposeNote, setTrackSource, trackSource, newProject, emptyFxLane, blankColumn, makePattern, removeTrack, restoreTrack, readProject, resizePattern, commandValue, duplicateTrack } from './project';

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
  expect(keyboardNote('q', 4)).toBe('C-5'); expect(keyboardNote('u', 4)).toBe('B-5'); expect(keyboardNote('i', 4)).toBe('C-6'); expect(keyboardNote(',', 4)).toBe('C-5');
  expect(keyboardNote('m', 8)).toBe('B-8'); expect(keyboardNote('q', 8)).toBeNull();
  expect(transposeNote('C-4', 1)).toBe('C#4'); expect(transposeNote('C-4', -1)).toBe('B-3'); expect(transposeNote('B-8', 1)).toBeNull(); expect(transposeNote('C-0', -1)).toBeNull(); expect(transposeNote('OFF', 1)).toBeNull();
  expect(keyboardNote('ArrowUp', 4)).toBeNull();
  expect(keyboardNote('', 4)).toBeNull();
  expect(keyboardNote('z', NaN)).toBeNull();
});

it('rejects invalid assignments and tempo before a project reaches playback', () => {
  const project = newProject();
  project.definitions.push({ id: 'soften', name: 'Filter', kind: 'fx', source: 'fx soften() { out = in }' });
  project.patterns[0].notes[0][0].inst[0] = 'soften';
  expect(() => readProject(JSON.stringify(project))).toThrow();
  project.patterns[0].notes[0][0].inst[0] = 'tone';
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
  project.patterns[0].fx[0][0].target = { kind: 'instrument', instrument: 'tone', parameter: 'cutoff' };
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
  bad(p => { p.master.volume_db = 20; }); bad(p => { (p.tracks[0] as { solo: unknown }).solo = 'yes'; });
  bad(p => { p.master.fx[0].definition = 'missing'; }); bad(p => { delete (p as { master?: unknown }).master; });
});

it('keeps returns, outputs and send devices, and rejects bad routing or too many returns', () => {
  const project = newProject();
  const verb = { name: 'Verb', fx: [], pan: 0, muted: false, volume_db: 0 };
  const addReturn = (track: typeof verb) => { project.returns.push(track); for (const p of project.patterns) p.return_fx.push([emptyFxLane(p.rows)]); };
  addReturn(verb); project.tracks[0].output = { kind: 'return', index: 0 };
  expect(readProject(JSON.stringify(project))).toEqual(project);
  project.definitions.push({ id: 'fx_a', name: 'A', kind: 'fx', source: 'fx fx_a { out = in }' }); const fx = { id: 'd1', definition: 'fx_a', bypass: false, values: {}, modulations: {} };
  const bad = (change: (p: typeof project) => void) => { const copy = structuredClone(project); change(copy); expect(() => readProject(JSON.stringify(copy))).toThrow(); };
  expect(readProject(JSON.stringify({ ...project, tracks: [{ ...project.tracks[0], fx: [{ ...fx, target: 3 }] }, ...project.tracks.slice(1)] }))).toBeTruthy();
  bad(p => { p.tracks[0].output = { kind: 'return', index: 8 }; });
  const second = { ...verb, name: 'Echo' }; addReturn(second);
  expect(readProject(JSON.stringify({ ...project, returns: [{ ...verb, output: { kind: 'return', index: 1 }, fx: [{ ...fx, target: 1 }] }, second] }))).toBeTruthy();
  bad(p => { p.returns[1].output = { kind: 'return', index: 0 }; });
  bad(p => { p.returns[0].output = { kind: 'return', index: 0 }; });
  bad(p => { p.returns[1].fx = [{ ...fx, target: 1 }]; });
  bad(p => { p.master.output = { kind: 'none' }; });
  bad(p => { p.tracks[0].fx = [{ ...fx, target: 0, pre: 'yes' as unknown as boolean }]; });
  expect(readProject(JSON.stringify({ ...project, tracks: [{ ...project.tracks[0], fx: [{ ...fx, target: 0, pre: true, mute_source: true }] }, ...project.tracks.slice(1)] }))).toBeTruthy();
  project.returns.pop(); for (const p of project.patterns) p.return_fx.pop();
  bad(p => { p.tracks[0].fx = [{ ...fx, target: 8 }]; });
  bad(p => { p.master.fx = [{ ...fx, target: 0 }]; });
  project.returns = Array(9).fill(verb); for (const p of project.patterns) p.return_fx = Array.from({ length: 9 }, () => [emptyFxLane(p.rows)]);
  expect(() => readProject(JSON.stringify(project))).toThrow();
  const { returns: _, ...old } = newProject();
  expect(readProject(JSON.stringify(old)).returns).toEqual([]);
});

it('keeps the matrix as small as it can be, and validates it', () => {
  const project = newProject(); const other = makePattern(project); project.sequence.push(other.id, project.patterns[0].id);
  expect(trackSource(project, 1, 2)).toBeNull();
  setTrackSource(project, 1, 2, { kind: 'mute' }); expect(project.matrix).toEqual([[], [null, null, { kind: 'mute' }]]);
  setTrackSource(project, 2, 0, { kind: 'pattern', id: other.id }); expect(trackSource(project, 2, 0)).toEqual({ kind: 'pattern', id: other.id });
  expect(readProject(JSON.stringify(project)).matrix).toEqual(project.matrix);
  setTrackSource(project, 1, 2, null); setTrackSource(project, 2, 0, null); expect(project.matrix).toBeUndefined();
  const read = (matrix: unknown) => () => readProject(JSON.stringify({ ...project, matrix }));
  expect(read([[null, { kind: 'mute' }]])).not.toThrow();
  for (const bad of [[[{ kind: 'pattern', id: 'missing' }]], [[], [], [], []], [[null, null, null, null, null]], [[{ kind: 'solo' }]], [['mute']], 'x']) expect(read(bad)).toThrow();
});

it("takes a deleted track's matrix cells with it and puts them back, shifting the tracks after it either way", () => {
  const project = newProject(); project.sequence.push(project.patterns[0].id);
  setTrackSource(project, 0, 1, { kind: 'mute' }); setTrackSource(project, 1, 2, { kind: 'mute' }); setTrackSource(project, 1, 3, { kind: 'pattern', id: project.patterns[0].id });
  const deleted = removeTrack(project, 1)!;
  expect(project.matrix).toEqual([[], [null, { kind: 'mute' }, { kind: 'pattern', id: project.patterns[0].id }]]);
  expect(restoreTrack(project, deleted)).toBe(true);
  expect(project.matrix).toEqual([[null, { kind: 'mute' }], [null, null, { kind: 'mute' }, { kind: 'pattern', id: project.patterns[0].id }]]);
  // If the sequence changed in between, the cells are not guessed at, but the other tracks still shift.
  const again = removeTrack(project, 1)!; project.sequence.push(project.patterns[0].id);
  expect(restoreTrack(project, again)).toBe(true);
  expect(project.matrix).toEqual([[], [null, null, { kind: 'mute' }, { kind: 'pattern', id: project.patterns[0].id }]]);
});

it('keeps parameter lanes for each return and the master, and rejects lanes that do not match the strips', () => {
  const project = newProject(), verb = { name: 'Verb', fx: [], pan: 0, muted: false, volume_db: 0 };
  project.returns.push(verb); for (const p of project.patterns) p.return_fx.push([emptyFxLane(p.rows)]);
  project.patterns[0].master_fx[0].values[2] = '-6'; project.patterns[0].return_fx[0][0].target = { kind: 'pan' };
  expect(readProject(JSON.stringify(project))).toEqual(project);
  const bad = (change: (p: typeof project) => void) => { const copy = structuredClone(project); change(copy); expect(() => readProject(JSON.stringify(copy))).toThrow(); };
  bad(p => { p.patterns[0].return_fx.pop(); }); // a lane list for every return
  bad(p => { p.patterns[0].master_fx = []; }); bad(p => { p.patterns[0].master_fx = Array.from({ length: 5 }, () => emptyFxLane(32)); });
  bad(p => { p.patterns[0].master_fx[0].target = { kind: 'instrument', instrument: 'tone', parameter: 'cutoff' }; }); // no notes, no instruments
  bad(p => { p.patterns[0].return_fx[0][0].values[0] = 'loud'; });
});

it('gives a new pattern lanes for the returns and the master, and copies them into a variation', () => {
  const project = newProject(); project.returns.push({ name: 'Verb', fx: [], pan: 0, muted: false, volume_db: 0 });
  for (const p of project.patterns) p.return_fx.push([emptyFxLane(p.rows)]);
  const blank = makePattern(project); expect(blank.return_fx).toHaveLength(1); expect(blank.master_fx).toHaveLength(1);
  project.patterns[0].master_fx[0].values[1] = '-3';
  const copy = makePattern(project, project.patterns[0]);
  expect(copy.master_fx[0].values[1]).toBe('-3'); copy.master_fx[0].values[1] = '0'; expect(project.patterns[0].master_fx[0].values[1]).toBe('-3');
  resizePattern(copy, 40); expect(copy.master_fx[0].values).toHaveLength(40); expect(copy.return_fx[0][0].values).toHaveLength(40);
});

it('starts a new project with a pattern that has nothing in it, and an instrument ready to play', () => {
  const project = emptyProject();
  expect(project.patterns[0].notes.flat().every(column => column.notes.every(n => n === null) && column.inst.every(i => i === null))).toBe(true);
  expect(project.definitions.some(d => d.kind === 'instr')).toBe(true); expect(project.tracks.map(t => t.name)).toEqual(['Track 01', 'Track 02', 'Track 03', 'Track 04']);
  expect(readProject(JSON.stringify(project))).toEqual(project);
  expect(newProject().patterns[0].notes[0][0].notes[0]).toBe('C-4'); // the phrase the tests use is still there
});

it('keeps note commands in the project and accepts older files that have none', () => {
  expect(commandValue('g8')).toBe('G8'); expect(commandValue(' O50 ')).toBe('O50'); expect(commandValue('V')).toBe('V'); expect(commandValue('R4')).toBe('R4'); expect(commandValue('D12')).toBe('D12');
  for (const bad of ['', 'G', 'O', 'V1', 'R1', 'R33', 'U0', 'X3', '8', 'G100']) expect(commandValue(bad), bad).toBeNull();
  const project = newProject();
  project.patterns[0].notes[0][0].cmd[3] = 'G8';
  expect(readProject(JSON.stringify(project)).patterns[0].notes[0][0].cmd[3]).toBe('G8');
  project.patterns[0].notes[0][0].cmd[3] = 'G';
  expect(() => readProject(JSON.stringify(project))).toThrow();
  const old = JSON.parse(JSON.stringify(newProject()));
  for (const column of old.patterns[0].notes.flat()) delete column.cmd;
  expect(readProject(JSON.stringify(old)).patterns[0].notes[0][0].cmd).toEqual([]);
  const longer = newProject();
  resizePattern(longer.patterns[0], 64);
  expect(longer.patterns[0].notes[0][0].cmd).toHaveLength(64);
});

it('duplicates a track with its effects, notes, lanes and matrix cells, right after it', () => {
  const project = newProject();
  project.tracks[1].fx.push({ id: 'd1', definition: 'x', bypass: false, values: { amount: 2 }, modulations: {} });
  project.tracks[1].solo = true;
  project.patterns[0].notes[1][0].notes[3] = 'E-3';
  project.patterns[0].fx[1][0].values[4] = '-6';
  setTrackSource(project, 0, 1, { kind: 'mute' });
  setTrackSource(project, 0, 2, { kind: 'pattern', id: project.patterns[0].id });
  expect(duplicateTrack(project, 1)).toBe(2);
  expect(project.tracks.map(t => t.name)).toEqual(['Lead', 'Bass', 'Bass copy', 'Drums', 'Texture']);
  const copy = project.tracks[2];
  expect(copy.fx).toEqual(project.tracks[1].fx); expect(copy.fx).not.toBe(project.tracks[1].fx); expect(copy.solo).toBe(false);
  const [pattern] = project.patterns;
  expect(pattern.notes[2][0].notes[3]).toBe('E-3'); expect(pattern.notes[2][0]).not.toBe(pattern.notes[1][0]);
  expect(pattern.fx[2][0].values[4]).toBe('-6'); expect(pattern.notes).toHaveLength(5); expect(pattern.fx).toHaveLength(5);
  // the later track keeps its cell, and the copy plays the same as the original
  expect(trackSource(project, 0, 1)).toEqual({ kind: 'mute' }); expect(trackSource(project, 0, 2)).toEqual({ kind: 'mute' });
  expect(trackSource(project, 0, 3)).toEqual({ kind: 'pattern', id: pattern.id });
  while (project.tracks.length < 16) duplicateTrack(project, 0);
  expect(duplicateTrack(project, 0)).toBeNull();
});

it('roundtrips timing, copies ramps independently, and rejects invalid timing', () => {
  const project = newProject();
  project.patterns[0].timing = { bpm: 120, rows_per_beat: 4, beats_per_bar: 7, beat_unit: 8, tempo: [{ row: 0, value: 120 }, { row: 32, value: 60 }] };
  expect(readProject(JSON.stringify(project))).toEqual(project);
  const copy = makePattern(project, project.patterns[0]);
  copy.timing!.tempo[0].value = 90;
  expect(project.patterns[0].timing.tempo[0].value).toBe(120);
  for (const timing of [ { ...copy.timing!, bpm: 0 }, { ...copy.timing!, beat_unit: 3 }, { ...copy.timing!, tempo: [{ row: 33, value: 100 }] }, { ...copy.timing!, tempo: [{ row: 2, value: 100 }, { row: 1, value: 100 }] } ]) {
    copy.timing = timing; expect(() => readProject(JSON.stringify(project))).toThrow();
  }
});

it('integrates BPM ramps and locates fractional rows for the playhead', () => {
  const t = { bpm: 120, rows_per_beat: 4, beats_per_bar: 7, beat_unit: 8, tempo: [{ row: 0, value: 120 }, { row: 4, value: 60 }] };
  expect(patternSeconds(t, 4)).toBeCloseTo(Math.log(2), 9);
  expect(patternRow(t, patternSeconds(t, 2.5), 32)).toBeCloseTo(2.5, 5);
  t.tempo = [{ row: 2, value: 60 }];
  expect(patternSeconds(t, 4)).toBeCloseTo(0.75, 9);
});
