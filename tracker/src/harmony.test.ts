import { expect, it } from 'vitest';
import { chordNotes, chords, scales, snapToScale } from './harmony';

const steps = (list: typeof scales, name: string) => list.find(([n]) => n === name)![1];

it('snaps notes to the nearest note of a scale, a tie going down, and leaves notes in it and non-notes alone', () => {
  const major = steps(scales, 'major');
  expect(['C#4', 'D#4', 'F-4', 'B-3', 'C-5'].map(note => snapToScale(note, 0, major))).toEqual(['C-4', 'D-4', 'F-4', 'B-3', 'C-5']);
  // a scale rooted off C: A minor pentatonic is A C D E G
  expect(['F-3', 'D#3', 'C-2'].map(note => snapToScale(note, 9, steps(scales, 'minor_pentatonic')))).toEqual(['E-3', 'D-3', 'C-2']);
  expect(snapToScale('OFF', 0, major)).toBeNull();
  // the lowest and highest notes stay inside the range
  expect(snapToScale('C#0', 0, major)).toBe('C-0');
});

it('builds a chord up from a note and drops notes past the top of the range', () => {
  expect(chordNotes('C-4', steps(chords, 'minor7'))).toEqual(['C-4', 'D#4', 'G-4', 'A#4']);
  expect(chordNotes('A-8', steps(chords, 'major'))).toEqual(['A-8']);
  expect(chordNotes('OFF', [0, 4, 7])).toBeNull();
});
