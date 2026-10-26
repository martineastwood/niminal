import { describe, expect, it } from 'vitest';
import { nameFor, peaksOf, relativePath, snippetFor } from './samples';

describe('naming samples in a piece', () => {
  it('writes a path from the folder of the file', () => {
    expect(relativePath('/music/set', '/music/set/drums/kick.wav')).toBe('drums/kick.wav');
    expect(relativePath('/music/set', '/music/loops/amen.wav')).toBe('../loops/amen.wav');
    expect(relativePath('/music/set/', '/music/set/kick.wav')).toBe('kick.wav');
  });

  it('writes the full path for a file that has not been saved', () => {
    expect(relativePath(null, 'C:\\samples\\kick.wav')).toBe('C:/samples/kick.wav');
  });

  it('makes a name that a binding can have', () => {
    expect(nameFor('/x/Kick 01.wav')).toBe('kick_01');
    expect(nameFor('/x/808.wav')).toBe('s_808');
  });
});

describe('what to write at the cursor', () => {
  it('binds the sample on an empty line', () => {
    expect(snippetFor('  ', '', 'drums/kick.wav')).toBe('kick = sample("drums/kick.wav")');
  });

  it('writes only the call where something is being assigned', () => {
    expect(snippetFor('snare = ', '', 'snare.wav')).toBe('sample("snare.wav")');
    expect(snippetFor('x(', ')', 'snare.wav')).toBe('sample("snare.wav")');
  });
});

describe('drawing a waveform', () => {
  it('keeps the quietest and loudest sample in each column', () => {
    expect(peaksOf(new Float32Array([0.5, -0.25, 1, -1]), 2)).toEqual([
      [-0.25, 0.5],
      [-1, 1],
    ]);
  });
});
