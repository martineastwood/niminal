import { noteAt, noteNumber } from './project';

/** Scales as semitones above the root, as the language's `in_scale` and `snap_to` read them. */
export const scales: [name: string, steps: number[]][] = [
  ['major', [0, 2, 4, 5, 7, 9, 11]], ['minor', [0, 2, 3, 5, 7, 8, 10]], ['harmonic_minor', [0, 2, 3, 5, 7, 8, 11]], ['melodic_minor', [0, 2, 3, 5, 7, 9, 11]],
  ['dorian', [0, 2, 3, 5, 7, 9, 10]], ['phrygian', [0, 1, 3, 5, 7, 8, 10]], ['lydian', [0, 2, 4, 6, 7, 9, 11]], ['mixolydian', [0, 2, 4, 5, 7, 9, 10]],
  ['locrian', [0, 1, 3, 5, 6, 8, 10]], ['major_pentatonic', [0, 2, 4, 7, 9]], ['minor_pentatonic', [0, 3, 5, 7, 10]], ['blues', [0, 3, 5, 6, 7, 10]],
  ['whole_tone', [0, 2, 4, 6, 8, 10]], ['chromatic', [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]],
];

/** Chords as semitones above the root, as the language's `chord` reads them. */
export const chords: [name: string, steps: number[]][] = [
  ['major', [0, 4, 7]], ['minor', [0, 3, 7]], ['dim', [0, 3, 6]], ['aug', [0, 4, 8]], ['sus2', [0, 2, 7]], ['sus4', [0, 5, 7]], ['power', [0, 7]],
  ['major6', [0, 4, 7, 9]], ['minor6', [0, 3, 7, 9]], ['major7', [0, 4, 7, 11]], ['minor7', [0, 3, 7, 10]], ['dom7', [0, 4, 7, 10]],
  ['dim7', [0, 3, 6, 9]], ['half_dim7', [0, 3, 6, 10]], ['major9', [0, 4, 7, 11, 14]], ['minor9', [0, 3, 7, 10, 14]],
];

/** `note` moved to the nearest note of the scale on `root` (a pitch class, 0 for C); halfway between two it goes down. Null for OFF, CUT and the like. */
export function snapToScale(note: string, root: number, steps: number[]): string | null {
  const number = noteNumber(note);
  if (number === null) return null;
  const octave = Math.floor((number - root) / 12);
  let best = number, distance = Infinity;
  for (let o = octave - 1; o <= octave + 1; o++) for (const step of steps) {
    const candidate = root + 12 * o + step, away = Math.abs(candidate - number);
    if (away < distance || away === distance && candidate < best) { best = candidate; distance = away; }
  }
  return noteAt(best);
}

/** The notes of a chord on `note`, the root first; null for anything that is not a note, and notes that would pass B-8 are left out. */
export function chordNotes(note: string, steps: number[]): string[] | null {
  const number = noteNumber(note);
  return number === null ? null : steps.map(step => noteAt(number + step)).filter((chordNote): chordNote is string => chordNote !== null);
}
