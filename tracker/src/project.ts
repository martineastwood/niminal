import tone from './presets/tone.nml?raw';

/** `values` are an instrument's parameter settings: notes start with them, unless a lane has changed them. */
export interface Definition { id: string; name: string; kind: 'instr' | 'fx'; source: string; values?: Record<string, number>; /** Parameters of an instrument whose sliders are hidden; lanes can still target them. */ hidden?: string[] }
export interface Lfo { enabled: boolean; shape: 'sine' | 'triangle' | 'saw' | 'square'; period: number; min: number; max: number }
/**
 * One effect in a track's chain. Its `id` is stable across reordering; lanes refer to it. A device with a
 * `target` is a send: its output feeds that return and the chain carries on past it. A return's must be a later return; the master has none.
 * Sends tap the strip after its fader unless `pre`, which taps them at their place in the chain; a `pre` send with
 * `mute_source` silences the chain after it.
 */
export interface Device { id: string; definition: string; bypass: boolean; values: Record<string, number>; modulations: Record<string, Lfo>; target?: number; pre?: boolean; mute_source?: boolean }
export const maxDevices = 8;
export const maxReturns = 8;
/** Where a track's or return's signal goes: the master, a return (a return can only feed a later one), or nowhere, leaving only its sends. */
export type Output = { kind: 'master' } | { kind: 'return'; index: number } | { kind: 'none' };
export interface Track { output?: Output; solo?: boolean; colour?: string; show?: Partial<Record<SubColumn, boolean>>; name: string; fx: Device[]; muted: boolean; volume_db: number; pan: number }
export interface Parameter { name: string; min: number; max: number; default: number; unit: string | null }
export type EffectParameters = Record<string, Parameter[]>;
export type FxTarget = { kind: 'volume' } | { kind: 'pan' } | { kind: 'effect'; device: string; parameter: string } | { kind: 'instrument'; instrument: string; parameter: string };
export interface EnvelopePoint { row: number; value: number }
export interface Envelope { enabled: boolean; mode: 'hold' | 'linear'; points: EnvelopePoint[] }
export interface FxLane { target: FxTarget | null; values: (string | null)[]; envelope?: Envelope }
export const emptyFxLane = (rows: number): FxLane => ({ target: { kind: 'volume' }, values: Array(rows).fill(null) });
export const cloneLane = (lane: FxLane): FxLane => ({ target: lane.target && { ...lane.target }, values: [...lane.values], ...(lane.envelope ? { envelope: structuredClone(lane.envelope) } : {}) });
/**
 * The columns of the pattern grid: each track, then each return, then the master. Returns and the master hold only
 * parameter lanes. A column's number is its place in that order.
 */
export const columnCount = (project: Song) => project.tracks.length + project.returns.length + 1;
/** The track, return or master a column of the grid belongs to. */
export const columnStrip = (project: Song, column: number): Track => column < project.tracks.length ? project.tracks[column] : column < project.tracks.length + project.returns.length ? project.returns[column - project.tracks.length] : project.master;
/** The parameter lanes of a column of the grid, in a pattern. */
export const columnLanes = (project: Song, pattern: Pattern, column: number): FxLane[] => column < project.tracks.length ? pattern.fx[column] : column < project.tracks.length + project.returns.length ? pattern.return_fx[column - project.tracks.length] : pattern.master_fx;
/**
 * One note column: parallel cells for each row. `notes` hold `C-4`, `OFF` (release) or `CUT` (stop at once); `inst` an instrument's
 * definition id; `vol` and `pan` a number from 0 to 128 (as text); `delay` from 0 to 255 256ths of a row; `start`, `chance`, `pitch`
 * and `rtg` are the note's expression (see {@link startValue}, {@link chanceValue}, {@link pitchValue} and {@link retriggerValue}).
 */
export interface NoteColumn { notes: (string | null)[]; inst: (string | null)[]; vol: (string | null)[]; pan: (string | null)[]; delay: (string | null)[]; start: (string | null)[]; chance: (string | null)[]; pitch: (string | null)[]; rtg: (string | null)[]; len: (string | null)[]; fx: (string | null)[] }
export type SubColumn = 'inst' | 'vol' | 'pan' | 'delay' | 'start' | 'chance' | 'pitch' | 'rtg' | 'len' | 'fx';
export const subColumns: SubColumn[] = ['inst', 'vol', 'pan', 'delay', 'start', 'chance', 'pitch', 'rtg', 'len', 'fx'];
/** A start cell's text, or null if it is not one: `50` starts the sample half way in, `<` plays it backwards, `50<` does both. */
export function startValue(text: string): string | null {
  const t = text.trim();
  return /^(\d{1,2})?<?$/.test(t) && t !== '' ? t : null;
}
/** A pitch cell's text, or null: `G8` glides the held note to this one over eight eighths of a row, `U12` / `D3` slide it up or down that many semitones during the row. */
export function pitchValue(text: string): string | null {
  const t = text.trim().toUpperCase(), n = Number(t.slice(1));
  return /^([GUD])\d{1,2}$/.test(t) && (t[0] === 'G' || n >= 1) ? t : null;
}
/** A retrigger cell's text, or null: the number of times the note plays in its row, 2 to 32, then optionally `:` and how much each hit's level changes from the one before, -100 to 100 percent (`4:-25`). */
export function retriggerValue(text: string): string | null {
  const t = text.trim(), match = /^(\d{1,2})(?::(-?\d{1,3}))?$/.exec(t);
  return match && Number(match[1]) >= 2 && Number(match[1]) <= 32 && Math.abs(Number(match[2] ?? 0)) <= 100 ? t : null;
}
/** A chance cell's text, or null: the percent chance, 0 to 100, that the note plays. */
export function chanceValue(text: string): string | null {
  const t = text.trim();
  return /^\d{1,3}$/.test(t) && Number(t) <= 100 ? String(Number(t)) : null;
}
/** A length cell's text, or null: how long the note sounds, 1 to 99 percent of a row. */
export function lenValue(text: string): string | null {
  const t = text.trim();
  return /^\d{1,2}$/.test(t) && Number(t) >= 1 ? String(Number(t)) : null;
}
/** An effect cell's text, or null: `V-20` or `V+10` move the held note's level by that many percent of full level, 1 to 99. */
export function fxValue(text: string): string | null {
  const t = text.trim().toUpperCase();
  return /^V[+-]\d{1,2}$/.test(t) && Number(t.slice(2)) >= 1 ? t : null;
}
/** Whether a row's pitch and retrigger cells compete: a glide keeps one voice and a retrigger restarts it. */
export const glideRetriggers = (pitch: string | null | undefined, rtg: string | null | undefined) => !!pitch && !!rtg && pitch[0] === 'G';
export const maxNoteColumns = 8;
export const blankColumn = (rows: number): NoteColumn => ({ notes: Array(rows).fill(null), inst: Array(rows).fill(null), vol: Array(rows).fill(null), pan: Array(rows).fill(null), delay: Array(rows).fill(null), start: Array(rows).fill(null), chance: Array(rows).fill(null), pitch: Array(rows).fill(null), rtg: Array(rows).fill(null), len: Array(rows).fill(null), fx: Array(rows).fill(null) });
export const cloneColumn = (column: NoteColumn): NoteColumn => ({ notes: [...column.notes], inst: [...column.inst], vol: [...column.vol], pan: [...column.pan], delay: [...column.delay], start: [...column.start], chance: [...column.chance], pitch: [...column.pitch], rtg: [...column.rtg], len: [...column.len], fx: [...column.fx] });
/** BPM counts quarter notes, LPB counts rows per quarter note; meter groups those rows. */
export interface PatternTiming { bpm: number; rows_per_beat: number; beats_per_bar: number; beat_unit: number; tempo: EnvelopePoint[] }
export function patternTiming(project: Song, pattern: Pattern): PatternTiming {
  return pattern.timing ?? { bpm: project.bpm, rows_per_beat: project.rows_per_beat, beats_per_bar: 4, beat_unit: 4, tempo: [] };
}
/** The swing, in percent, a pattern plays with: its own, or else the song's. */
export const patternSwing = (project: Song, pattern: Pattern) => pattern.swing ?? project.swing ?? 0;
/**
 * Elapsed seconds to a fractional pattern row of a pattern `rows` long, integrating linear BPM ramps. Swing moves that
 * percent of the shorter row's time from the second row of each pair to the first (the engine does the same sum);
 * pairs begin at row 0 and an odd last row is left alone.
 */
export function patternSeconds(t: PatternTiming, row: number, swing = 0, rows = Infinity): number {
  if (swing <= 0 || row >= rows) return straightSeconds(t, row);
  const first = Math.floor(row) & ~1;
  if (first + 1 >= rows) return straightSeconds(t, row);
  const start = straightSeconds(t, first), a = straightSeconds(t, first + 1) - start, b = straightSeconds(t, first + 2) - straightSeconds(t, first + 1), moved = Math.min(a, b) * swing / 100;
  return row < first + 1 ? start + (straightSeconds(t, row) - start) * (a + moved) / a : start + a + moved + (straightSeconds(t, row) - straightSeconds(t, first + 1)) * (b - moved) / b;
}
function straightSeconds(t: PatternTiming, row: number): number {
  let at = 0, seconds = 0;
  while (at < row) {
    const before = t.tempo.filter(p => p.row <= at).at(-1), after = t.tempo.find(p => p.row > at);
    const end = Math.min(row, after?.row ?? row);
    const bpm = (r: number) => before ? after ? before.value + (after.value - before.value) * (r - before.row) / (after.row - before.row) : before.value : t.bpm;
    const a = bpm(at), b = bpm(end);
    seconds += 60 / t.rows_per_beat * (Math.abs(b-a) < 1e-9 ? (end-at)/a : (end-at)*Math.log(b/a)/(b-a));
    at = end;
  }
  return seconds;
}
export function patternRow(t: PatternTiming, seconds: number, rows: number, swing = 0): number {
  let lo = 0, hi = rows;
  for (let i = 0; i < 24; i++) { const mid = (lo+hi)/2; if (patternSeconds(t, mid, swing, rows) < seconds) lo = mid; else hi = mid; }
  return (lo+hi)/2;
}
export const validSwing = (swing: unknown) => swing === undefined || typeof swing === 'number' && Number.isFinite(swing) && swing >= 0 && swing <= 75;
export function validTiming(t: PatternTiming, rows: number): boolean {
  return !!t && Number.isFinite(t.bpm) && t.bpm >= 20 && t.bpm <= 300 && Number.isInteger(t.rows_per_beat) && t.rows_per_beat >= 1 && t.rows_per_beat <= 16 &&
    Number.isInteger(t.beats_per_bar) && t.beats_per_bar >= 1 && t.beats_per_bar <= 32 && [1, 2, 4, 8, 16].includes(t.beat_unit) && Array.isArray(t.tempo) && t.tempo.length <= 257 &&
    t.tempo.every((p, i) => !!p && Number.isFinite(p.row) && p.row >= 0 && p.row <= rows && Number.isFinite(p.value) && p.value >= 20 && p.value <= 300 && (i === 0 || p.row > t.tempo[i-1].row));
}
export interface Pattern {
  timing?: PatternTiming;
  /** Percent, 0 to 75, instead of the song's. */
  swing?: number;
  id: string; name: string; rows: number; notes: NoteColumn[][];
  /** Parameter lanes by track, then by return, then the master's. Returns and the master have no notes. */
  fx: FxLane[][]; return_fx: FxLane[][]; master_fx: FxLane[];
}
/** Song playback repeats these sequence positions (inclusive) once it reaches them, until stopped. */
export interface LoopRange { start: number; end: number }
/** Where one track plays from in one sequence step, when not from the step's own pattern: silent, or another pattern's track. */
export type TrackSource = { kind: 'mute' } | { kind: 'pattern'; id: string };
export interface Song { bpm: number; rows_per_beat: number; /** Percent, 0 to 75; absent is straight. */ swing?: number; tracks: Track[]; master: Track; returns: Track[]; patterns: Pattern[]; sequence: string[]; loop_range?: LoopRange;
  /** By sequence step, then track. Steps and tracks beyond the ends of the lists, and `null`, play the step's own pattern. */
  matrix?: (TrackSource | null)[][]; instrument_values?: Record<string, Record<string, number>> }
/** A named group of consecutive sequence steps, from `start` up to the next section's start. It only organises the sequence list. */
export interface Section { start: number; name: string; collapsed?: boolean }
export interface Project extends Song { version: 12; id: string; definitions: Definition[]; sections?: Section[] }
export const storageKey = 'niminal-tracker-project-v12';

/** A project with a short four-note phrase on the first track, so there is something to play. */
export function newProject(): Project {
  return {
    version: 12, id: crypto.randomUUID(), bpm: 125, rows_per_beat: 4,
    definitions: [
      { id: 'tone', name: 'Warm saw', kind: 'instr', source: tone },
    ],
    returns: [],
    master: { name: 'Master', fx: [], pan: 0, muted: false, volume_db: 0 },
    tracks: ['Lead', 'Bass', 'Drums', 'Texture'].map(name => ({ name, fx: [], pan: 0, muted: false, volume_db: 0 })),
    patterns: [{ fx: Array.from({ length: 4 }, () => [emptyFxLane(32)]), return_fx: [], master_fx: [emptyFxLane(32)], id: 'pattern_00', name: 'First idea', rows: 32, notes: Array.from({ length: 4 }, (_, track) => [{ ...blankColumn(32), notes: Array.from({ length: 32 }, (_, row) => track === 0 && row % 4 === 0 ? ['C-4', 'D#4', 'G-4', 'A#4'][Math.floor(row / 4) % 4] : null), inst: Array.from({ length: 32 }, (_, row) => track === 0 && row % 4 === 0 ? 'tone' : null) }]) }],
    sequence: ['pattern_00'],
  };
}

/** The project the app starts with: four tracks named Track 01 to Track 04, the first instrument, and a pattern with nothing in it yet. */
export function emptyProject(): Project {
  const project = newProject();
  project.tracks.forEach((track, index) => { track.name = `Track ${String(index + 1).padStart(2, '0')}`; });
  for (const column of project.patterns[0].notes.flat()) { column.notes.fill(null); column.inst.fill(null); }
  return project;
}

export function makePattern(project: Project, source?: Pattern): Pattern {
  const pattern: Pattern = {
    id: `pattern_${crypto.randomUUID().replaceAll('-', '_')}`,
    name: source ? `${source.name} variation` : `Pattern ${project.patterns.length.toString().padStart(2, '0')}`,
    rows: source?.rows ?? 32,
    ...(source?.timing ? { timing: structuredClone(source.timing) } : {}),
    ...(source?.swing !== undefined ? { swing: source.swing } : {}),
    fx: source ? source.fx.map(lanes => lanes.map(cloneLane)) : Array.from({ length: project.tracks.length }, () => [emptyFxLane(32)]),
    return_fx: source ? source.return_fx.map(lanes => lanes.map(cloneLane)) : project.returns.map(() => [emptyFxLane(32)]),
    master_fx: source ? source.master_fx.map(cloneLane) : [emptyFxLane(32)],
    notes: source ? source.notes.map(track => track.map(cloneColumn)) : Array.from({ length: project.tracks.length }, (_, track) => Array.from({ length: project.patterns[0]?.notes[track]?.length ?? 1 }, () => blankColumn(32))),
  };
  project.patterns.push(pattern);
  return pattern;
}

export function resizePattern(pattern: Pattern, rows: number) {
  pattern.rows = Math.min(256, Math.max(1, Math.trunc(rows) || 1));
  if (pattern.timing) pattern.timing.tempo = pattern.timing.tempo.filter(p => p.row <= pattern.rows);
  // Retain notes beyond a shortened pattern so increasing its length restores them.
  const cells = [...pattern.notes.flat().flatMap(c => [c.notes, c.inst, c.vol, c.pan, c.delay, c.start, c.chance, c.pitch, c.rtg, c.len, c.fx]), ...pattern.fx.flat().map(lane => lane.values), ...pattern.return_fx.flat().map(lane => lane.values), ...pattern.master_fx.map(lane => lane.values)];
  for (const column of cells) while (column.length < pattern.rows) column.push(null);
}

export function readProject(raw: string): Project {
  const value = JSON.parse(raw);
  if (value && typeof value === 'object') value.returns ??= [];
  for (const pattern of Array.isArray(value?.patterns) ? value.patterns : []) for (const column of Array.isArray(pattern?.notes) ? pattern.notes.flat() : []) {
    if (column && typeof column === 'object') { column.len ??= Array(pattern.rows).fill(null); column.fx ??= Array(pattern.rows).fill(null); }
  }
  if (value?.version !== 12 || typeof value.id !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/.test(value.id) || !Array.isArray(value.definitions) || value.definitions.length === 0 ||
      !value.definitions.every((d: Definition) => d && typeof d.id === 'string' && /^[a-zA-Z_][\w-]*$/.test(d.id) && typeof d.name === 'string' && typeof d.source === 'string' && ['instr', 'fx'].includes(d.kind) && (d.values === undefined || !!d.values && typeof d.values === 'object' && Object.values(d.values).every(Number.isFinite)) && (d.hidden === undefined || Array.isArray(d.hidden) && d.hidden.every((name: unknown) => typeof name === 'string'))) ||
      new Set(value.definitions.map((d: Definition) => d.id)).size !== value.definitions.length ||
      !Number.isFinite(value.bpm) || value.bpm < 20 || value.bpm > 300 || !validSwing(value.swing) ||
      !Number.isInteger(value.rows_per_beat) || value.rows_per_beat < 1 || value.rows_per_beat > 16 ||
      !Array.isArray(value.tracks) || (value.tracks.length < 1 || value.tracks.length > 16) ||
      !value.tracks.every((t: Track) => validTrack(t, value.definitions)) || !validTrack(value.master, value.definitions) ||
      !Array.isArray(value.returns) || value.returns.length > maxReturns || !value.returns.every((t: Track) => validTrack(t, value.definitions)) ||
      (value.master.output && value.master.output.kind !== 'master' || value.master.fx.some((d: Device) => d.target !== undefined)) ||
      value.returns.some((t: Track, i: number) => t.output?.kind === 'return' && t.output.index <= i || t.fx.some(d => d.target !== undefined && d.target <= i)) ||
      !Array.isArray(value.patterns) || value.patterns.length < 1 || value.patterns.length > 128 ||
      !value.patterns.every((p: Pattern) => p && typeof p.id === 'string' && /^[a-zA-Z_][\w-]*$/.test(p.id) && typeof p.name === 'string' &&
        Number.isInteger(p.rows) && p.rows >= 1 && p.rows <= 256 && (p.timing === undefined || validTiming(p.timing, p.rows)) && validSwing(p.swing) && Array.isArray(p.notes) && p.notes.length === value.tracks.length &&
        p.notes.every(track => Array.isArray(track) && track.length >= 1 && track.length <= maxNoteColumns && track.every(column => validColumn(column, p.rows, value.definitions))) &&
        Array.isArray(p.fx) && p.fx.length === value.tracks.length && p.fx.every(lanes => validLanes(lanes, p.rows)) &&
        Array.isArray(p.return_fx) && p.return_fx.length === value.returns.length && [...p.return_fx, p.master_fx].every(lanes => validLanes(lanes, p.rows) && lanes.every(lane => lane.target?.kind !== 'instrument'))) ||
      new Set(value.patterns.map((p: Pattern) => p.id)).size !== value.patterns.length ||
      !Array.isArray(value.sequence) || value.sequence.length < 1 || value.sequence.length > 1024 ||
      !value.sequence.every((id: unknown) => value.patterns.some((p: Pattern) => p.id === id)) ||
      value.sections !== undefined && !(Array.isArray(value.sections) && value.sections.every((s: Section, i: number) => !!s && Number.isInteger(s.start) && s.start >= 0 && s.start < value.sequence.length && (i === 0 || s.start > value.sections[i - 1].start) && typeof s.name === 'string' && s.name.length <= 60 && (s.collapsed === undefined || typeof s.collapsed === 'boolean'))) ||
      value.matrix !== undefined && !(Array.isArray(value.matrix) && value.matrix.length <= value.sequence.length && value.matrix.every((step: unknown) => Array.isArray(step) && step.length <= value.tracks.length && step.every((s: TrackSource | null) => s === null || !!s && (s.kind === 'mute' || s.kind === 'pattern' && value.patterns.some((p: Pattern) => p.id === s.id))))) ||
      value.loop_range !== undefined && !(Number.isInteger(value.loop_range?.start) && Number.isInteger(value.loop_range.end) && value.loop_range.start >= 0 && value.loop_range.start <= value.loop_range.end && value.loop_range.end < value.sequence.length)) {
    throw new Error('Not a supported niminal tracker project (expected version 12)');
  }
  return value;
}

const noteNames = ['C-', 'C#', 'D-', 'D#', 'E-', 'F-', 'F#', 'G-', 'G#', 'A-', 'A#', 'B-'];
/** The laptop keyboard as a piano: the Z row plays from `octave`, the Q row from the octave above, with the black keys on the rows between. */
const keyboard = ['zsxdcvgbhnjm,l.;/', 'q2w3er5t6y7ui9o0p'];
export function keyboardNote(key: string, octave: number): string | null {
  if (key.length !== 1 || !Number.isInteger(octave) || octave < 0 || octave > 8) return null;
  const lower = keyboard[0].indexOf(key.toLowerCase()), upper = keyboard[1].indexOf(key.toLowerCase());
  const semitone = lower >= 0 ? lower : upper >= 0 ? 12 + upper : -1;
  return semitone < 0 ? null : transposeNote(`C-${octave}`, semitone);
}

/** A note's place on the keyboard counted in semitones from C-0, or null for OFF, CUT and anything else that is not a note. */
export function noteNumber(note: string): number | null {
  const match = /^([A-G][#-])([0-8])$/.exec(note), index = match ? noteNames.indexOf(match[1]) : -1;
  return match && index >= 0 ? Number(match[2]) * 12 + index : null;
}
/** The note at `number` semitones from C-0, or null outside C-0 to B-8. */
export function noteAt(number: number): string | null {
  return Number.isInteger(number) && number >= 0 && number <= 107 ? `${noteNames[number % 12]}${Math.floor(number / 12)}` : null;
}
/** A note moved by some semitones, or null for OFF, anything else that is not a note, or a result outside C-0 to B-8. */
export function transposeNote(note: string, semitones: number): string | null {
  const number = noteNumber(note);
  return number === null ? null : noteAt(number + semitones);
}

export const trackSource = (project: Song, step: number, track: number): TrackSource | null => project.matrix?.[step]?.[track] ?? null;

/** Sets where a track plays from in a step, keeping the matrix as small as it can be. */
export function setTrackSource(project: Song, step: number, track: number, source: TrackSource | null) {
  const matrix = project.matrix ??= [];
  while (matrix.length <= step) matrix.push([]);
  while (matrix[step].length <= track) matrix[step].push(null);
  matrix[step][track] = source;
  tidyMatrix(project);
}

/** Drops empty steps from the end and the whole matrix when nothing is set. */
export function tidyMatrix(project: Song) {
  const matrix = project.matrix; if (!matrix) return;
  for (const step of matrix) while (step.length && step[step.length - 1] === null) step.pop();
  while (matrix.length && !matrix[matrix.length - 1].length) matrix.pop();
  if (!matrix.length) delete project.matrix;
}

export interface DeletedTrack { index: number; track: Track; notes: Map<string, NoteColumn[]>; fx: Map<string, FxLane[]>; matrix: (TrackSource | null)[]; sequence: string[] }

export function removeTrack(project: Project, index: number): DeletedTrack | null {
  if (project.tracks.length <= 1 || index < 0 || index >= project.tracks.length) return null;
  // The track's matrix cells go with it, by step.
  const matrix = project.sequence.map((_, step) => trackSource(project, step, index));
  for (const step of project.matrix ?? []) if (index < step.length) step.splice(index, 1);
  tidyMatrix(project);
  return { index, track: project.tracks.splice(index, 1)[0], matrix, sequence: [...project.sequence], notes: new Map(project.patterns.map(p => [p.id, p.notes.splice(index, 1)[0]])), fx: new Map(project.patterns.map(p => [p.id, p.fx.splice(index, 1)[0]])) };
}

/**
 * Adds a copy of a track right after it: its mixer settings and effect chain, and its notes, parameter lanes and
 * matrix cells in every pattern. Returns the copy's index, or null when the song has no room for another track.
 */
export function duplicateTrack(project: Project, index: number): number | null {
  if (project.tracks.length >= 16 || index < 0 || index >= project.tracks.length) return null;
  const copy: Track = { ...structuredClone(project.tracks[index]), name: `${project.tracks[index].name} copy`, solo: false };
  project.tracks.splice(index + 1, 0, copy);
  for (const pattern of project.patterns) {
    pattern.notes.splice(index + 1, 0, pattern.notes[index].map(cloneColumn));
    pattern.fx.splice(index + 1, 0, pattern.fx[index].map(cloneLane));
  }
  project.sequence.forEach((_, step) => {
    const source = trackSource(project, step, index), row = project.matrix?.[step];
    if (source === null && (!row || row.length <= index + 1)) return;
    const matrix = project.matrix ??= []; while (matrix.length <= step) matrix.push([]);
    while (matrix[step].length < index + 1) matrix[step].push(null);
    matrix[step].splice(index + 1, 0, source && structuredClone(source));
  });
  tidyMatrix(project);
  return index + 1;
}

export function restoreTrack(project: Project, deleted: DeletedTrack): boolean {
  if (project.tracks.length >= 16) return false;
  project.tracks.splice(deleted.index, 0, deleted.track);
  // Tracks after it move along in every step. Its own cells come back only if the sequence is as it was.
  const same = deleted.sequence.length === project.sequence.length && deleted.sequence.every((id, step) => id === project.sequence[step]);
  project.sequence.forEach((_, step) => {
    const source = same ? deleted.matrix[step] : null, row = project.matrix?.[step];
    if (source === null && (!row || row.length <= deleted.index)) return;
    const matrix = project.matrix ??= []; while (matrix.length <= step) matrix.push([]);
    while (matrix[step].length < deleted.index) matrix[step].push(null);
    matrix[step].splice(deleted.index, 0, source);
  });
  tidyMatrix(project);
  for (const pattern of project.patterns) {
    const restored = (deleted.notes.get(pattern.id) ?? [blankColumn(pattern.rows)]).map(cloneColumn);
    for (const column of restored) for (const cells of [column.notes, column.inst, column.vol, column.pan, column.delay, column.start, column.chance, column.pitch, column.rtg, column.len, column.fx]) while (cells.length < pattern.rows) cells.push(null);
    pattern.notes.splice(deleted.index, 0, restored);
    const fx = (deleted.fx.get(pattern.id) ?? [emptyFxLane(pattern.rows)]).map(cloneLane);
    for (const column of fx) while (column.values.length < pattern.rows) column.values.push(null);
    pattern.fx.splice(deleted.index, 0, fx);
  }
  return true;
}

/** One track's, return's or the master's parameter lanes in a pattern of `rows` rows. */
function validLanes(lanes: FxLane[], rows: number) {
  return Array.isArray(lanes) && lanes.length >= 1 && lanes.length <= 4 && lanes.every(column => column && validEnvelope(column.envelope) && validTarget(column.target) && Array.isArray(column.values) && column.values.length >= rows && column.values.length <= 256 && column.values.every(v => v === null || typeof v === 'string' && v.trim() !== '' && Number.isFinite(Number(v))) && (column.target !== null || column.values.every(v => v === null) && !column.envelope?.points.length));
}
export function validTarget(target: FxTarget | null): boolean {
  return target === null || !!target && (target.kind === 'volume' || target.kind === 'pan' || target.kind === 'effect' && validDeviceId(target.device) && typeof target.parameter === 'string' && /^[a-zA-Z_]\w*$/.test(target.parameter) || target.kind === 'instrument' && typeof target.instrument === 'string' && typeof target.parameter === 'string' && /^[a-zA-Z_]\w*$/.test(target.parameter));
}
const integerWithin = (max: number) => (v: unknown) => v === null || typeof v === 'string' && /^\d{1,3}$/.test(v) && Number(v) <= max;
function validColumn(column: NoteColumn, rows: number, definitions: Definition[]) {
  const sized = (cells: unknown, optional: boolean): cells is (string | null)[] => Array.isArray(cells) && cells.length <= 256 && (cells.length >= rows || optional && cells.length === 0);
  return !!column && sized(column.notes, false) && sized(column.inst, true) && sized(column.vol, true) && sized(column.pan, true) && sized(column.delay, true) && sized(column.start, true) && sized(column.chance, true) && sized(column.pitch, true) && sized(column.rtg, true) && sized(column.len, true) && sized(column.fx, true) &&
    column.notes.every(n => n === null || typeof n === 'string' && (n === 'OFF' || n === 'CUT' || /^[A-G][#-][0-8]$/.test(n))) &&
    column.inst.every(id => id === null || typeof id === 'string' && definitions.some(d => d.id === id && d.kind === 'instr')) &&
    column.vol.every(integerWithin(128)) && column.pan.every(integerWithin(128)) && column.delay.every(integerWithin(255)) && column.start.every(c => c === null || startValue(c) === c) && column.chance.every(c => c === null || chanceValue(c) === c) && column.pitch.every(c => c === null || pitchValue(c) === c) && column.rtg.every(c => c === null || retriggerValue(c) === c) && column.len.every(c => c === null || lenValue(c) === c) && column.fx.every(c => c === null || fxValue(c) === c) &&
    !column.pitch.some((c, row) => glideRetriggers(c, column.rtg[row]));
}
function validTrack(t: Track, definitions: Definition[]): boolean {
  return !!(
    t && typeof t.name === 'string' && (t.colour === undefined || /^#[0-9a-f]{6}$/i.test(t.colour)) &&
    Array.isArray(t.fx) && t.fx.length <= maxDevices && t.fx.every((d, i) => !!d && validDeviceId(d.id) && t.fx.findIndex(other => other.id === d.id) === i && typeof d.bypass === 'boolean' && definitions.some((def: Definition) => def.id === d.definition && def.kind === 'fx') && !!d.values && Object.values(d.values).every(Number.isFinite) && !!d.modulations && Object.values(d.modulations).every(validLfo)) &&
    Number.isFinite(t.pan) && t.pan >= -1 && t.pan <= 1 &&
    (t.output === undefined || validOutput(t.output)) && t.fx.every(d => (d.target === undefined || Number.isInteger(d.target) && d.target >= 0 && d.target < maxReturns) && [d.pre, d.mute_source].every(flag => flag === undefined || typeof flag === 'boolean')) &&
    (t.solo === undefined || typeof t.solo === 'boolean') && typeof t.muted === 'boolean' && Number.isFinite(t.volume_db) && t.volume_db >= -60 && t.volume_db <= 6
  );
}
const validOutput = (o: Output) => !!o && (o.kind === 'master' || o.kind === 'none' || o.kind === 'return' && Number.isInteger(o.index) && o.index >= 0 && o.index < maxReturns);
export const validDeviceId = (id: unknown) => typeof id === 'string' && /^[a-zA-Z0-9_]{1,40}$/.test(id);
function validEnvelope(envelope: Envelope | undefined) {
  return envelope === undefined || !!envelope && typeof envelope.enabled === 'boolean' && ['hold', 'linear'].includes(envelope.mode) && Array.isArray(envelope.points) && envelope.points.length <= 4097 && envelope.points.every((p, i) => p && Number.isFinite(p.row) && p.row >= 0 && p.row <= 256 && Number.isFinite(p.value) && (i === 0 || p.row > envelope.points[i - 1].row));
}
function validLfo(value: Lfo) {
  return value && typeof value.enabled === 'boolean' && ['sine', 'triangle', 'saw', 'square'].includes(value.shape) &&
    Number.isFinite(value.period) && value.period >= 1 && value.period <= 256 &&
    Number.isFinite(value.min) && Number.isFinite(value.max) && value.min >= 0 && value.max <= 1 && value.min <= value.max;
}
