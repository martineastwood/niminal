import tone from './presets/tone.nml?raw';
import soften from './presets/soften.nml?raw';

export interface Definition { id: string; name: string; kind: 'instr' | 'fx'; source: string }
export interface Track { name: string; instrument: string | null; effect: string | null; muted: boolean; volume_db: number; notes: (string | null)[] }
export interface Pattern { bpm: number; rows_per_beat: number; tracks: Track[] }
export interface Project extends Pattern { version: 2; definitions: Definition[] }
export const storageKey = 'niminal-tracker-project-v2';

export function newProject(): Project {
  return {
    version: 2, bpm: 125, rows_per_beat: 4,
    definitions: [
      { id: 'tone', name: 'Warm saw', kind: 'instr', source: tone },
      { id: 'soften', name: 'Low pass', kind: 'fx', source: soften },
    ],
    tracks: ['Lead', 'Bass', 'Drums', 'Texture'].map((name, track) => ({
      name, instrument: track === 0 ? 'tone' : null, effect: null, muted: false, volume_db: 0,
      notes: Array.from({ length: 32 }, (_, row) => track === 0 && row % 4 === 0 ? ['C-4', 'D#4', 'G-4', 'A#4'][Math.floor(row / 4) % 4] : null),
    })),
  };
}

export function readProject(raw: string): Project {
  const value = JSON.parse(raw);
  if (value.version !== 2 || !Array.isArray(value.definitions) || value.definitions.length === 0 ||
      !value.definitions.every((d: Definition) => d && typeof d.id === 'string' && /^[a-zA-Z_][\w-]*$/.test(d.id) && typeof d.name === 'string' && typeof d.source === 'string' && ['instr', 'fx'].includes(d.kind)) ||
      new Set(value.definitions.map((d: Definition) => d.id)).size !== value.definitions.length ||
      !Number.isFinite(value.bpm) || value.bpm < 20 || value.bpm > 300 ||
      !Number.isInteger(value.rows_per_beat) || value.rows_per_beat < 1 || value.rows_per_beat > 16 ||
      !Array.isArray(value.tracks) || value.tracks.length !== 4 ||
      !value.tracks.every((t: Track) => t && typeof t.name === 'string' &&
        (t.instrument === null || value.definitions.some((d: Definition) => d.id === t.instrument && d.kind === 'instr')) &&
        (t.effect === null || value.definitions.some((d: Definition) => d.id === t.effect && d.kind === 'fx')) &&
        typeof t.muted === 'boolean' && Number.isFinite(t.volume_db) && t.volume_db >= -60 && t.volume_db <= 6 &&
        Array.isArray(t.notes) && t.notes.length === 32 && t.notes.every(n => n === null || typeof n === 'string' && /^[A-G][#-][0-8]$/.test(n)))) {
    throw new Error('Not a supported niminal tracker project (expected version 2)');
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
