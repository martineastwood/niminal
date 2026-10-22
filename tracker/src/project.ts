import tone from './presets/tone.nml?raw';
import soften from './presets/soften.nml?raw';
export interface Definition { id: string; name: string; kind: 'instr' | 'fx'; source: string }
export interface Project { version: 1; definitions: Definition[]; tracks: (string | null)[][] }
export const storageKey = 'niminal-tracker-project-v1';

export function newProject(): Project {
  return {
    version: 1,
    definitions: [
      { id: 'tone', name: 'Warm saw', kind: 'instr', source: tone },
      { id: 'soften', name: 'Low pass', kind: 'fx', source: soften },
    ],
    tracks: Array.from({ length: 4 }, (_, track) => Array.from({ length: 32 }, (_, row) => track === 0 && row % 4 === 0 ? ['C-4', 'D#4', 'G-4', 'A#4'][Math.floor(row / 4) % 4] : null)),
  };
}

export function readProject(raw: string): Project {
  const value = JSON.parse(raw);
  if (value.version !== 1 || !Array.isArray(value.definitions) || value.definitions.length === 0 ||
      !value.definitions.every((d: Definition) => d && typeof d.id === 'string' && /^[a-zA-Z_][\w-]*$/.test(d.id) && typeof d.name === 'string' && typeof d.source === 'string' && ['instr', 'fx'].includes(d.kind)) ||
      new Set(value.definitions.map((d: Definition) => d.id)).size !== value.definitions.length ||
      !Array.isArray(value.tracks) || value.tracks.length !== 4 ||
      !value.tracks.every((t: unknown) => Array.isArray(t) && t.length === 32 && t.every(n => n === null || typeof n === 'string' && /^[A-G][#-][0-8]$/.test(n)))) {
    throw new Error('Not a supported niminal tracker project');
  }
  return value;
}

const keyboard = 'zsxdcvgbhnjm';
export function keyboardNote(key: string, octave: number): string | null {
  const semitone = keyboard.indexOf(key.toLowerCase());
  if (semitone < 0) return null;
  return ['C-', 'C#', 'D-', 'D#', 'E-', 'F-', 'F#', 'G-', 'G#', 'A-', 'A#', 'B-'][semitone] + octave;
}
