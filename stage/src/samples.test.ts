// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { dropInto, dropText, isAudioFile, nameFor, relativePath, snippetFor } from './samples';

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

describe('what to write where a file is dropped', () => {
  it('binds the sample on an empty line', () => {
    expect(snippetFor('  ', '', 'drums/kick.wav')).toBe('kick = sample("drums/kick.wav")');
  });

  it('writes only the call where something is being assigned', () => {
    expect(snippetFor('snare = ', '', 'snare.wav')).toBe('sample("snare.wav")');
    expect(snippetFor('x(', ')', 'snare.wav')).toBe('sample("snare.wav")');
  });

  it('binds several files, each on its own line', () => {
    expect(dropText(['/set/kick.wav', '/set/Snare 2.flac'], '/set', '', '')).toBe(
      'kick = sample("kick.wav")\nsnare_2 = sample("Snare 2.flac")',
    );
  });

  it('knows audio files by their extension, whatever the case', () => {
    expect(['a.wav', 'B.FLAC', 'c.m4a'].every(isAudioFile)).toBe(true);
    expect(['a.nml', 'notes.txt', 'wav'].some(isAudioFile)).toBe(false);
  });
});

describe('dropping files on the code', () => {
  const editor = (doc: string) => new EditorView({ state: EditorState.create({ doc }) });

  it('binds one file dropped on an empty line, and calls it where something is being assigned', () => {
    const view = editor('a = 1\n\nsnare = ');
    expect(dropInto(view, ['/set/kick.wav'], 6, '/set')).toBe('inserted sample("kick.wav")');
    expect(view.state.doc.toString()).toBe('a = 1\nkick = sample("kick.wav")\nsnare = ');
    dropInto(view, ['/set/snare.wav'], view.state.doc.length, '/set');
    expect(view.state.doc.line(3).text).toBe('snare = sample("snare.wav")');
  });

  it('puts several files on lines above the one they land on', () => {
    const view = editor('a = 1\nb = 2');
    expect(dropInto(view, ['/set/kick.wav', '/set/hat.wav'], 8, '/set')).toBe('inserted 2 samples');
    expect(view.state.doc.toString()).toBe('a = 1\nkick = sample("kick.wav")\nhat = sample("hat.wav")\nb = 2');
  });

  it('uses the cursor when it is not told where, and ignores what is not audio', () => {
    const view = editor('x = ');
    view.dispatch({ selection: { anchor: 4 } });
    expect(dropInto(view, ['/set/notes.txt'], null, '/set')).toBeNull();
    dropInto(view, ['/set/a.wav'], null, '/set');
    expect(view.state.doc.toString()).toBe('x = sample("a.wav")');
  });
});
