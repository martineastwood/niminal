import tone from './presets/tone.nml?raw';

export interface Definition { id: string; name: string; kind: 'instr' | 'fx'; source: string }
export interface Lfo { enabled: boolean; shape: 'sine' | 'triangle' | 'saw' | 'square'; period: number; min: number; max: number }
export interface TrackFx { definition: string; values: Record<string, number>; modulations: Record<string, Lfo> }
export interface Track { colour?: string; name: string; instrument: string | null; fx: TrackFx | null; muted: boolean; volume_db: number; pan: number }
export interface Parameter { name: string; min: number; max: number; default: number; unit: string | null }
export type EffectParameters = Record<string, Parameter[]>;
export type FxTarget = { kind: 'volume' } | { kind: 'pan' } | { kind: 'effect'; parameter: string };
export interface EnvelopePoint { row: number; value: number }
export interface Envelope { enabled: boolean; mode: 'hold' | 'linear'; points: EnvelopePoint[] }
export interface FxLane { target: FxTarget | null; values: (string | null)[]; envelope?: Envelope }
export const emptyFxLane = (rows: number): FxLane => ({ target: { kind: 'volume' }, values: Array(rows).fill(null) });
export interface Pattern { id: string; name: string; rows: number; notes: (string | null)[][]; fx: FxLane[][] }
export interface Song { bpm: number; rows_per_beat: number; tracks: Track[]; patterns: Pattern[]; sequence: string[] }
export interface Project extends Song { version: 6; definitions: Definition[] }
export const storageKey = 'niminal-tracker-project-v6';

export function newProject(): Project {
  return {
    version: 6, bpm: 125, rows_per_beat: 4,
    definitions: [
      { id: 'tone', name: 'Warm saw', kind: 'instr', source: tone },
    ],
    tracks: ['Lead', 'Bass', 'Drums', 'Texture'].map((name, track) => ({ name, instrument: track === 0 ? 'tone' : null, fx: null, pan: 0, muted: false, volume_db: 0 })),
    patterns: [{ fx: Array.from({ length: 4 }, () => [emptyFxLane(32)]), id: 'pattern_00', name: 'First idea', rows: 32, notes: Array.from({ length: 4 }, (_, track) => Array.from({ length: 32 }, (_, row) => track === 0 && row % 4 === 0 ? ['C-4', 'D#4', 'G-4', 'A#4'][Math.floor(row / 4) % 4] : null)) }],
    sequence: ['pattern_00'],
  };
}

export function makePattern(project: Project, source?: Pattern): Pattern {
  const pattern: Pattern = {
    id: `pattern_${crypto.randomUUID().replaceAll('-', '_')}`,
    name: source ? `${source.name} variation` : `Pattern ${project.patterns.length.toString().padStart(2, '0')}`,
    rows: source?.rows ?? 32,
    fx: source ? source.fx.map(track => track.map(column => ({ target: column.target && { ...column.target }, values: [...column.values], ...(column.envelope ? { envelope: structuredClone(column.envelope) } : {}) }))) : Array.from({ length: project.tracks.length }, () => [emptyFxLane(32)]),
    notes: source ? source.notes.map(track => [...track]) : Array.from({ length: project.tracks.length }, () => Array(32).fill(null)),
  };
  project.patterns.push(pattern);
  return pattern;
}

export function resizePattern(pattern: Pattern, rows: number) {
  pattern.rows = Math.min(256, Math.max(1, Math.trunc(rows) || 1));
  // Retain notes beyond a shortened pattern so increasing its length restores them.
  for (const column of [...pattern.notes, ...pattern.fx.flat().map(lane => lane.values)]) while (column.length < pattern.rows) column.push(null);
}

export function readProject(raw: string): Project {
  const value = JSON.parse(raw);
  if (value?.version !== 6 || !Array.isArray(value.definitions) || value.definitions.length === 0 ||
      !value.definitions.every((d: Definition) => d && typeof d.id === 'string' && /^[a-zA-Z_][\w-]*$/.test(d.id) && typeof d.name === 'string' && typeof d.source === 'string' && ['instr', 'fx'].includes(d.kind)) ||
      new Set(value.definitions.map((d: Definition) => d.id)).size !== value.definitions.length ||
      !Number.isFinite(value.bpm) || value.bpm < 20 || value.bpm > 300 ||
      !Number.isInteger(value.rows_per_beat) || value.rows_per_beat < 1 || value.rows_per_beat > 16 ||
      !Array.isArray(value.tracks) || (value.tracks.length < 1 || value.tracks.length > 16) ||
      !value.tracks.every((t: Track) => t && typeof t.name === 'string' && (t.colour === undefined || /^#[0-9a-f]{6}$/i.test(t.colour)) &&
        (t.instrument === null || value.definitions.some((d: Definition) => d.id === t.instrument && d.kind === 'instr')) &&
        (t.fx === null || !!t.fx && value.definitions.some((d: Definition) => d.id === t.fx!.definition && d.kind === 'fx') && t.fx.values && Object.values(t.fx.values).every(Number.isFinite) && t.fx.modulations && Object.values(t.fx.modulations).every(validLfo)) &&
        Number.isFinite(t.pan) && t.pan >= -1 && t.pan <= 1 &&
        typeof t.muted === 'boolean' && Number.isFinite(t.volume_db) && t.volume_db >= -60 && t.volume_db <= 6) ||
      !Array.isArray(value.patterns) || value.patterns.length < 1 || value.patterns.length > 128 ||
      !value.patterns.every((p: Pattern) => p && typeof p.id === 'string' && /^[a-zA-Z_][\w-]*$/.test(p.id) && typeof p.name === 'string' &&
        Number.isInteger(p.rows) && p.rows >= 1 && p.rows <= 256 && Array.isArray(p.notes) && p.notes.length === value.tracks.length &&
        p.notes.every(track => Array.isArray(track) && track.length >= p.rows && track.length <= 256 && track.every(n => n === null || typeof n === 'string' && /^[A-G][#-][0-8]$/.test(n))) &&
        Array.isArray(p.fx) && p.fx.length === value.tracks.length && p.fx.every(track => Array.isArray(track) && track.length >= 1 && track.length <= 4 && track.every(column => column && validEnvelope(column.envelope) && validTarget(column.target) && Array.isArray(column.values) && column.values.length >= p.rows && column.values.length <= 256 && column.values.every(v => v === null || typeof v === 'string' && v.trim() !== '' && Number.isFinite(Number(v))) && (column.target !== null || column.values.every(v => v === null) && !column.envelope?.points.length)))) ||
      new Set(value.patterns.map((p: Pattern) => p.id)).size !== value.patterns.length ||
      !Array.isArray(value.sequence) || value.sequence.length < 1 || value.sequence.length > 1024 ||
      !value.sequence.every((id: unknown) => value.patterns.some((p: Pattern) => p.id === id))) {
    throw new Error('Not a supported niminal tracker project (expected version 6)');
  }
  return value;
}

const keyboard = 'zsxdcvgbhnjm';
export function keyboardNote(key: string, octave: number): string | null {
  if (key.length !== 1 || !Number.isInteger(octave) || octave < 0 || octave > 8) return null;
  const semitone = keyboard.indexOf(key.toLowerCase());
  if (semitone < 0) return null;
  return ['C-', 'C#', 'D-', 'D#', 'E-', 'F-', 'F#', 'G-', 'G#', 'A-', 'A#', 'B-'][semitone] + octave;
}

export interface DeletedTrack { index: number; track: Track; notes: Map<string, (string | null)[]>; fx: Map<string, FxLane[]> }

export function removeTrack(project: Project, index: number): DeletedTrack | null {
  if (project.tracks.length <= 1 || index < 0 || index >= project.tracks.length) return null;
  return { index, track: project.tracks.splice(index, 1)[0], notes: new Map(project.patterns.map(p => [p.id, p.notes.splice(index, 1)[0]])), fx: new Map(project.patterns.map(p => [p.id, p.fx.splice(index, 1)[0]])) };
}

export function restoreTrack(project: Project, deleted: DeletedTrack): boolean {
  if (project.tracks.length >= 16) return false;
  project.tracks.splice(deleted.index, 0, deleted.track);
  for (const pattern of project.patterns) {
    const restored = [...(deleted.notes.get(pattern.id) ?? [])];
    while (restored.length < pattern.rows) restored.push(null);
    pattern.notes.splice(deleted.index, 0, restored);
    const fx = (deleted.fx.get(pattern.id) ?? [emptyFxLane(pattern.rows)]).map(column => ({ target: column.target && { ...column.target }, values: [...column.values], ...(column.envelope ? { envelope: structuredClone(column.envelope) } : {}) }));
    for (const column of fx) while (column.values.length < pattern.rows) column.values.push(null);
    pattern.fx.splice(deleted.index, 0, fx);
  }
  return true;
}

export function validTarget(target: FxTarget | null): boolean {
  return target === null || !!target && (target.kind === 'volume' || target.kind === 'pan' || target.kind === 'effect' && typeof target.parameter === 'string' && /^[a-zA-Z_]\w*$/.test(target.parameter));
}
function validEnvelope(envelope: Envelope | undefined) {
  return envelope === undefined || !!envelope && typeof envelope.enabled === 'boolean' && ['hold', 'linear'].includes(envelope.mode) && Array.isArray(envelope.points) && envelope.points.length <= 4097 && envelope.points.every((p, i) => p && Number.isFinite(p.row) && p.row >= 0 && p.row <= 256 && Number.isFinite(p.value) && (i === 0 || p.row > envelope.points[i - 1].row));
}
function validLfo(value: Lfo) {
  return value && typeof value.enabled === 'boolean' && ['sine', 'triangle', 'saw', 'square'].includes(value.shape) &&
    Number.isFinite(value.period) && value.period >= 1 && value.period <= 256 &&
    Number.isFinite(value.min) && Number.isFinite(value.max) && value.min >= 0 && value.max <= 1 && value.min <= value.max;
}
