import { patternSeconds, patternRow } from './project';
import { expect, it } from 'vitest';
import { emptyProject, keyboardNote, transposeNote, setTrackSource, trackSource, newProject, emptyFxLane, blankColumn, makePattern, removeTrack, restoreTrack, readProject, resizePattern, patternSwing, commandValue, commandProblem, glideRetriggers, trackCommandValue, duplicateTrack } from './project';

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
  project.patterns[0].notes.push([blankColumn(32)]); project.patterns[0].fx.push([emptyFxLane()]);
  expect(readProject(JSON.stringify(project)).tracks).toHaveLength(5);
  while (project.tracks.length < 17) {
    project.tracks.push({ ...project.tracks[0] });
    project.patterns[0].notes.push([blankColumn(32)]); project.patterns[0].fx.push([emptyFxLane()]);
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
  project.patterns[0].fx[0][0].envelope = { enabled: true, mode: 'hold', points: [{ row: 4, value: 2400 }] };
  expect(readProject(JSON.stringify(project))).toEqual(project);
  const copy = makePattern(project, project.patterns[0]); copy.fx[0][0].envelope!.points[0].value = 800;
  expect(project.patterns[0].fx[0][0].envelope!.points[0].value).toBe(2400);
  resizePattern(copy, 2); resizePattern(copy, 64); expect(copy.fx[0][0].envelope!.points[0].value).toBe(800);
  project.patterns[0].fx[0][0].target = null; expect(() => readProject(JSON.stringify(project))).toThrow(); // an envelope needs a parameter
  project.patterns[0].fx[0][0].target = { kind: 'effect', device: 'd1', parameter: 'cutoff' };
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
  const addReturn = (track: typeof verb) => { project.returns.push(track); for (const p of project.patterns) { p.return_fx.push([emptyFxLane()]); p.commands.splice(project.tracks.length + project.returns.length - 1, 0, [Array(p.rows).fill(null)]); } };
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
  project.returns = Array(9).fill(verb); for (const p of project.patterns) p.return_fx = Array.from({ length: 9 }, () => [emptyFxLane()]);
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
  project.returns.push(verb); for (const p of project.patterns) { p.return_fx.push([emptyFxLane()]); p.commands.splice(project.tracks.length + project.returns.length - 1, 0, [Array(p.rows).fill(null)]); }
  project.patterns[0].master_fx[0].envelope = { enabled: true, mode: 'hold', points: [{ row: 2, value: -6 }] }; project.patterns[0].return_fx[0][0].target = { kind: 'pan' };
  expect(readProject(JSON.stringify(project))).toEqual(project);
  const bad = (change: (p: typeof project) => void) => { const copy = structuredClone(project); change(copy); expect(() => readProject(JSON.stringify(copy))).toThrow(); };
  bad(p => { p.patterns[0].return_fx.pop(); }); // a lane list for every return
  bad(p => { p.patterns[0].master_fx = []; }); bad(p => { p.patterns[0].master_fx = Array.from({ length: 17 }, () => emptyFxLane()); });
  bad(p => { p.patterns[0].master_fx[0].target = { kind: 'instrument', instrument: 'tone', parameter: 'cutoff' }; }); // no notes, no instruments
  bad(p => { p.patterns[0].return_fx[0][0].envelope = { enabled: true, mode: 'hold', points: [{ row: 1, value: 0 }, { row: 1, value: 1 }] }; }); // points run in order
});

it('gives a new pattern lanes for the returns and the master, and copies them into a variation', () => {
  const project = newProject(); project.returns.push({ name: 'Verb', fx: [], pan: 0, muted: false, volume_db: 0 });
  for (const p of project.patterns) p.return_fx.push([emptyFxLane()]);
  const blank = makePattern(project); expect(blank.return_fx).toHaveLength(1); expect(blank.master_fx).toHaveLength(1);
  project.patterns[0].master_fx[0].envelope = { enabled: true, mode: 'hold', points: [{ row: 1, value: -3 }] };
  const copy = makePattern(project, project.patterns[0]);
  expect(copy.master_fx[0].envelope!.points[0].value).toBe(-3); copy.master_fx[0].envelope!.points[0].value = 0; expect(project.patterns[0].master_fx[0].envelope!.points[0].value).toBe(-3);
});

it('starts a new project with a pattern that has nothing in it, and an instrument ready to play', () => {
  const project = emptyProject();
  expect(project.patterns[0].notes.flat().every(column => column.notes.every(n => n === null) && column.inst.every(i => i === null))).toBe(true);
  expect(project.definitions.some(d => d.kind === 'instr')).toBe(true); expect(project.tracks.map(t => t.name)).toEqual(['Track 01', 'Track 02', 'Track 03', 'Track 04']);
  expect(readProject(JSON.stringify(project))).toEqual(project);
  expect(newProject().patterns[0].notes[0][0].notes[0]).toBe('C-4'); // the phrase the tests use is still there
});

it('reads commands as a letter and two hex digits, and keeps command columns in the project', () => {
  expect(commandValue(' s80 ')).toBe('S80'); expect(commandValue('RF4')).toBe('RF4'); expect(commandValue('C06')).toBe('C06'); expect(commandValue('G10')).toBe('G10'); expect(commandValue('u00')).toBe('U00'); // 00 repeats the last value above it
  for (const bad of ['', 'S', 'S8', 'S800', 'SG0', '80S', 'Q10', 'B02', 'Y00', 'O81', 'R00', 'R07', 'C10', 'C00', 'C0C']) expect(commandValue(bad), bad).toBeNull();
  expect(commandProblem('Q10')).toContain('Commands are');
  expect(glideRetriggers(['G10', 'R03'])).toBe(true); expect(glideRetriggers(['U10', 'R03'])).toBe(false); expect(glideRetriggers([null, undefined])).toBe(false);
  const project = newProject(), column = project.patterns[0].notes[0][0];
  column.fx = [Array(32).fill(null), Array(32).fill(null)];
  column.fx[0][3] = 'G10'; column.fx[0][2] = 'S80'; column.fx[1][4] = 'R03';
  const read = readProject(JSON.stringify(project)).patterns[0].notes[0][0];
  expect([read.fx[0][3], read.fx[0][2], read.fx[1][4]]).toEqual(['G10', 'S80', 'R03']);
  column.fx[0][2] = 'Z99'; expect(() => readProject(JSON.stringify(project))).toThrow(); column.fx[0][2] = null;
  column.fx[1][3] = 'R03'; expect(() => readProject(JSON.stringify(project))).toThrow(); // a glide and a retrigger compete, even in two columns
  column.fx[1][3] = null;
  column.fx.push(Array(32).fill(null), Array(32).fill(null), Array(32).fill(null)); expect(() => readProject(JSON.stringify(project))).toThrow(); // at most four
  expect(() => readProject(JSON.stringify({ ...newProject(), version: 12 }))).toThrow(/version 13/);
  const longer = newProject();
  longer.patterns[0].notes[0][0].fx = [Array(32).fill(null)];
  resizePattern(longer.patterns[0], 64);
  expect(longer.patterns[0].notes[0][0].fx[0]).toHaveLength(64);
});

it('keeps commands in the fx columns of a track and the command columns of each return and the master, and rejects ones that cannot play', () => {
  expect(trackCommandValue(' l80 ')).toBe('L80'); expect(trackCommandValue('PFF')).toBe('PFF'); expect(trackCommandValue('13ff')).toBe('13FF');
  expect(trackCommandValue('01ff')).toBe('01FF'); expect(trackCommandValue('08FF')).toBe('08FF'); expect(trackCommandValue('0cff')).toBe('0CFF'); // place 0 is a macro, 1 to F
  for (const bad of ['', 'L8', 'L800', 'Q80', 'S80', '0000', '1000', '12G0', '12F', '12FFF']) expect(trackCommandValue(bad), bad).toBeNull();
  expect(commandValue('13ff')).toBe('13FF'); expect(commandValue('s80')).toBe('S80'); expect(commandValue('l80')).toBe('L80'); // a track's fx column takes both kinds
  const project = newProject(), verb = { name: 'Verb', fx: [], pan: 0, muted: false, volume_db: 0 };
  project.returns.push(verb); for (const p of project.patterns) { p.return_fx.push([emptyFxLane()]); p.commands.splice(project.returns.length - 1, 0, [Array(p.rows).fill(null)]); }
  expect(project.patterns[0].commands).toHaveLength(2); // a return and the master
  expect(project.patterns[0].notes[0][0].fx).toHaveLength(1); // a track starts with a command column
  project.patterns[0].notes[0][0].fx[0][3] = '12FF'; project.patterns[0].commands[0][0][3] = 'L80'; project.patterns[0].commands[1][0][0] = 'P00';
  expect(readProject(JSON.stringify(project))).toEqual(project);
  const bad = (change: (p: typeof project) => void) => { const copy = structuredClone(project); change(copy); expect(() => readProject(JSON.stringify(copy))).toThrow(); };
  bad(p => { p.patterns[0].commands.pop(); }); // a list for every strip
  bad(p => { p.patterns[0].commands[0][0][3] = 'S80'; }); // a strip has no notes to start
  bad(p => { p.patterns[0].commands[0] = Array.from({ length: 5 }, () => Array(32).fill(null)); }); // at most four columns
  bad(p => { p.patterns[0].commands[0][0] = Array(31).fill(null); });
  bad(p => { p.patterns[0].commands[1] = []; }); // a return or the master keeps a column
  // a new pattern and a copy of a track carry them along
  const copy = makePattern(project, project.patterns[0]); expect(copy.commands[0][0][3]).toBe('L80'); expect(copy.notes[0][0].fx[0][3]).toBe('12FF');
  const at = duplicateTrack(project, 0)!; expect(project.patterns[0].notes[at][0].fx[0][3]).toBe('12FF');
  const removed = removeTrack(project, at)!; restoreTrack(project, removed); expect(project.patterns[0].notes[at][0].fx[0][3]).toBe('12FF');
  const longer = newProject(); resizePattern(longer.patterns[0], 64); expect(longer.patterns[0].commands[0][0]).toHaveLength(64); expect(longer.patterns[0].notes[0][0].fx[0]).toHaveLength(64);
});

it('duplicates a track with its effects, notes, lanes and matrix cells, right after it', () => {
  const project = newProject();
  project.tracks[1].fx.push({ id: 'd1', definition: 'x', bypass: false, values: { amount: 2 }, modulations: {} });
  project.tracks[1].solo = true;
  project.patterns[0].notes[1][0].notes[3] = 'E-3';
  project.patterns[0].fx[1][0].envelope = { enabled: true, mode: 'hold', points: [{ row: 4, value: -6 }] };
  setTrackSource(project, 0, 1, { kind: 'mute' });
  setTrackSource(project, 0, 2, { kind: 'pattern', id: project.patterns[0].id });
  expect(duplicateTrack(project, 1)).toBe(2);
  expect(project.tracks.map(t => t.name)).toEqual(['Lead', 'Bass', 'Bass copy', 'Drums', 'Texture']);
  const copy = project.tracks[2];
  expect(copy.fx).toEqual(project.tracks[1].fx); expect(copy.fx).not.toBe(project.tracks[1].fx); expect(copy.solo).toBe(false);
  const [pattern] = project.patterns;
  expect(pattern.notes[2][0].notes[3]).toBe('E-3'); expect(pattern.notes[2][0]).not.toBe(pattern.notes[1][0]);
  expect(pattern.fx[2][0].envelope!.points[0].value).toBe(-6); expect(pattern.notes).toHaveLength(5); expect(pattern.fx).toHaveLength(5);
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

it('swings pairs of rows without changing how long a pair lasts, and reads swing back', () => {
  const t = { bpm: 120, rows_per_beat: 4, beats_per_bar: 4, beat_unit: 4, tempo: [] }; // an eighth of a second a row
  expect([0, 1, 2, 3, 4].map(row => patternSeconds(t, row, 50, 4))).toEqual([0, 0.1875, 0.25, 0.4375, 0.5]); // 3 parts to 1
  expect(patternSeconds(t, 0.5, 50, 4)).toBeCloseTo(0.09375); // half way through a swung row is half its length
  expect(patternSeconds(t, 3, 50, 3)).toBeCloseTo(0.375); // an odd last row is left alone
  expect(patternSeconds(t, 1, 0, 4)).toBeCloseTo(0.125);
  const ramp = { ...t, tempo: [{ row: 0, value: 60 }, { row: 6, value: 180 }] };
  for (const pair of [0, 2, 4, 6]) expect(patternSeconds(ramp, pair, 40, 6)).toBeCloseTo(patternSeconds(ramp, pair, 0, 6));
  expect(patternRow(t, 0.1875, 4, 50)).toBeCloseTo(1, 3);
  const project = newProject();
  expect(patternSwing(project, project.patterns[0])).toBe(0);
  project.swing = 50; expect(patternSwing(project, project.patterns[0])).toBe(50);
  project.patterns[0].swing = 0; expect(patternSwing(project, project.patterns[0])).toBe(0); // its own, even when it is straight
  expect(readProject(JSON.stringify(project)).patterns[0].swing).toBe(0);
  project.swing = 76; expect(() => readProject(JSON.stringify(project))).toThrow();
  project.swing = 50; project.patterns[0].swing = -1; expect(() => readProject(JSON.stringify(project))).toThrow();
});
