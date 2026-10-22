import tone from './presets/tone.nml?raw';
import soften from './presets/soften.nml?raw';

export interface Definition { id: string; name: string; kind: 'instr' | 'fx'; source: string }
export interface Track { name: string; instrument: string | null; effect: string | null; muted: boolean; volume_db: number }
export interface Pattern { id: string; name: string; rows: number; notes: (string | null)[][] }
export interface Song { bpm: number; rows_per_beat: number; tracks: Track[]; patterns: Pattern[]; sequence: string[] }
export interface Project extends Song { version: 3; definitions: Definition[] }
export const storageKey = 'niminal-tracker-project-v3';

export function newProject(): Project {
  return {
    version: 3, bpm: 125, rows_per_beat: 4,
    definitions: [
      { id: 'tone', name: 'Warm saw', kind: 'instr', source: tone },
      { id: 'soften', name: 'Low pass', kind: 'fx', source: soften },
    ],
    tracks: ['Lead', 'Bass', 'Drums', 'Texture'].map((name, track) => ({ name, instrument: track === 0 ? 'tone' : null, effect: null, muted: false, volume_db: 0 })),
    patterns: [{ id: 'pattern_00', name: 'First idea', rows: 32, notes: Array.from({ length: 4 }, (_, track) => Array.from({ length: 32 }, (_, row) => track === 0 && row % 4 === 0 ? ['C-4', 'D#4', 'G-4', 'A#4'][Math.floor(row / 4) % 4] : null)) }],
    sequence: ['pattern_00'],
  };
}

export function makePattern(project: Project, source?: Pattern): Pattern {
  const pattern: Pattern = {
    id: `pattern_${crypto.randomUUID().replaceAll('-', '_')}`,
    name: source ? `${source.name} variation` : `Pattern ${project.patterns.length.toString().padStart(2, '0')}`,
    rows: source?.rows ?? 32,
    notes: source ? source.notes.map(track => [...track]) : Array.from({ length: project.tracks.length }, () => Array(32).fill(null)),
  };
  project.patterns.push(pattern);
  return pattern;
}

export function resizePattern(pattern: Pattern, rows: number) {
  pattern.rows = Math.min(256, Math.max(1, Math.trunc(rows) || 1));
  // Retain notes beyond a shortened pattern so increasing its length restores them.
  for (const track of pattern.notes) while (track.length < pattern.rows) track.push(null);
}

export function readProject(raw: string): Project {
  const value = JSON.parse(raw);
  if (value?.version !== 3 || !Array.isArray(value.definitions) || value.definitions.length === 0 ||
      !value.definitions.every((d: Definition) => d && typeof d.id === 'string' && /^[a-zA-Z_][\w-]*$/.test(d.id) && typeof d.name === 'string' && typeof d.source === 'string' && ['instr', 'fx'].includes(d.kind)) ||
      new Set(value.definitions.map((d: Definition) => d.id)).size !== value.definitions.length ||
      !Number.isFinite(value.bpm) || value.bpm < 20 || value.bpm > 300 ||
      !Number.isInteger(value.rows_per_beat) || value.rows_per_beat < 1 || value.rows_per_beat > 16 ||
      !Array.isArray(value.tracks) || (value.tracks.length < 1 || value.tracks.length > 16) ||
      !value.tracks.every((t: Track) => t && typeof t.name === 'string' &&
        (t.instrument === null || value.definitions.some((d: Definition) => d.id === t.instrument && d.kind === 'instr')) &&
        (t.effect === null || value.definitions.some((d: Definition) => d.id === t.effect && d.kind === 'fx')) &&
        typeof t.muted === 'boolean' && Number.isFinite(t.volume_db) && t.volume_db >= -60 && t.volume_db <= 6) ||
      !Array.isArray(value.patterns) || value.patterns.length < 1 || value.patterns.length > 128 ||
      !value.patterns.every((p: Pattern) => p && typeof p.id === 'string' && /^[a-zA-Z_][\w-]*$/.test(p.id) && typeof p.name === 'string' &&
        Number.isInteger(p.rows) && p.rows >= 1 && p.rows <= 256 && Array.isArray(p.notes) && p.notes.length === value.tracks.length &&
        p.notes.every(track => Array.isArray(track) && track.length >= p.rows && track.length <= 256 && track.every(n => n === null || typeof n === 'string' && /^[A-G][#-][0-8]$/.test(n)))) ||
      new Set(value.patterns.map((p: Pattern) => p.id)).size !== value.patterns.length ||
      !Array.isArray(value.sequence) || value.sequence.length < 1 || value.sequence.length > 1024 ||
      !value.sequence.every((id: unknown) => value.patterns.some((p: Pattern) => p.id === id))) {
    throw new Error('Not a supported niminal tracker project (expected version 3)');
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
