import { describe, expect, it } from 'vitest';
import { arrangementCommand, arrangementLabel, arrangementProgress, clock } from './arrangements';
import { Arrangement } from './live';

const song = (over: Partial<Arrangement> = {}): Arrangement => ({
  name: 'song',
  running: false,
  length: 150,
  elapsed: null,
  section: null,
  sections: [],
  ...over,
});

describe('an arrangement in the stage', () => {
  it('starts when idle and stops, letting go of the sound, when it plays', () => {
    expect(arrangementCommand(song())).toBe('song.start');
    expect(arrangementCommand(song({ running: true, elapsed: 3 }))).toBe('song.stop\nhush');
  });

  it('shows how far through it is', () => {
    expect(arrangementProgress(song())).toBe(0);
    expect(arrangementProgress(song({ running: true, elapsed: 75 }))).toBe(0.5);
    expect(arrangementProgress(song({ running: true, elapsed: 400 }))).toBe(1);
    expect(arrangementProgress(song({ running: true, elapsed: 5, length: 0 }))).toBe(0);
  });

  it('names the section it is in, and its length when idle', () => {
    expect(arrangementLabel(song())).toBe('2:30');
    expect(arrangementLabel(song({ running: true, elapsed: 65, section: 'chorus' }))).toBe('chorus · 1:05');
    expect(arrangementLabel(song({ running: true, elapsed: 9, length: 20 }))).toBe('0:09');
    expect(arrangementLabel(song({ length: 0 }))).toBe('');
  });

  it('writes seconds as minutes and seconds', () => {
    expect(clock(0)).toBe('0:00');
    expect(clock(59.6)).toBe('1:00');
    expect(clock(330)).toBe('5:30');
  });
});
