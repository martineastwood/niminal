import { expect, it } from 'vitest';
import { describe as reading } from './expression';

it('reads a cell back in plain words', () => {
  expect(reading('start', '50')).toBe('Starts 50% into the sample');
  expect(reading('start', '<')).toBe('Plays it backwards');
  expect(reading('start', '5<')).toBe('Starts 5% into the sample and plays it backwards');
  expect(reading('chance', '100')).toBe('Always plays'); expect(reading('chance', '0')).toBe('Never plays'); expect(reading('chance', '25')).toBe('Plays 25% of the time');
  expect(reading('pitch', 'G8')).toBe('Glides the held note to this one over 1 row'); expect(reading('pitch', 'G16')).toBe('Glides the held note to this one over 2 rows');
  expect(reading('pitch', 'U12')).toBe('Slides the held note up 12 semitones over the row'); expect(reading('pitch', 'D1')).toBe('Slides the held note down 1 semitone over the row');
  expect(reading('rtg', '4')).toBe('Plays 4 times in the row');
  expect(reading('rtg', '4:-25')).toBe('Plays 4 times in the row, each 25% quieter than the one before');
  expect(reading('rtg', '3:50')).toBe("Plays 3 times in the row, each 50% louder than the one before, ending at the note's level");
  expect(reading('rtg', '4:-100')).toBe('Plays 4 times in the row, then silent');
  expect(reading('rtg', '1')).toBeNull(); expect(reading('pitch', 'X3')).toBeNull();
});
