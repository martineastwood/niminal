import type { EditorView } from '@codemirror/view';

const slashes = (path: string) => path.replace(/\\/g, '/');

/** `file` as a path from folder `from`, the way a piece names its samples; the full path when there is no folder. */
export function relativePath(from: string | null, file: string) {
  const target = slashes(file);
  if (!from) return target;
  const base = slashes(from).replace(/\/$/, '').split('/');
  const parts = target.split('/');
  let shared = 0;
  while (shared < base.length && shared < parts.length - 1 && base[shared] === parts[shared]) shared++;
  if (shared === 0) return target;
  return [...Array<string>(base.length - shared).fill('..'), ...parts.slice(shared)].join('/');
}

/** A name a piece can bind a sample to: `Kick 01.wav` becomes `kick_01`. */
export function nameFor(file: string) {
  const stem = (slashes(file).split('/').pop() ?? file).replace(/\.[^.]*$/, '');
  const name = stem.toLowerCase().replace(/[^a-z0-9_]+/g, '_').replace(/^_+|_+$/g, '');
  return /^[a-z]/.test(name) ? name : `s_${name || 'sample'}`;
}

/**
 * What to write for a sample: just the call where something is already being assigned or passed
 * (`kick = ▮`), and a whole binding on an empty line.
 */
export function snippetFor(lineBefore: string, lineAfter: string, path: string) {
  const call = `sample("${path}")`;
  const blank = lineBefore.trim() === '' && lineAfter.trim() === '';
  return blank ? `${nameFor(path)} = ${call}` : call;
}

/** The kinds of audio file `sample(...)` can load, by extension (the engine's own list). */
const AUDIO = new Set(['wav', 'flac', 'mp3', 'ogg', 'oga', 'aif', 'aiff', 'aifc', 'm4a', 'mp4', 'aac', 'caf']);

export function isAudioFile(path: string) {
  const name = slashes(path).split('/').pop() ?? '';
  return name.includes('.') && AUDIO.has(name.split('.').pop()!.toLowerCase());
}

/**
 * What to write for audio files dropped on the code. One file goes where it was dropped, as a call or a whole
 * binding as `snippetFor` decides; several go on lines of their own, each bound to its name.
 */
export function dropText(files: string[], fileDir: string | null, lineBefore: string, lineAfter: string) {
  const paths = files.map(file => relativePath(fileDir, file));
  if (paths.length === 1) return snippetFor(lineBefore, lineAfter, paths[0]);
  return paths.map(path => `${nameFor(path)} = sample("${path}")`).join('\n');
}

/**
 * Write dropped audio files into the code at position `at` (the cursor if none). One file goes where it was
 * dropped; several go on lines above the one they were dropped on. Returns what to tell the player, or null if
 * none of the files was audio.
 */
export function dropInto(view: EditorView, files: string[], at: number | null, fileDir: string | null) {
  const audio = files.filter(isAudioFile);
  if (!audio.length) return null;
  const pos = Math.min(Math.max(at ?? view.state.selection.main.head, 0), view.state.doc.length);
  const line = view.state.doc.lineAt(pos);
  const text = dropText(audio, fileDir, line.text.slice(0, pos - line.from), line.text.slice(pos - line.from));
  const [from, insert] = audio.length === 1 ? [pos, text] : [line.from, `${text}\n`];
  view.dispatch({ changes: { from, insert }, selection: { anchor: from + insert.length } });
  view.focus();
  return audio.length === 1 ? `inserted ${text.replace(/^.* = /, '')}` : `inserted ${audio.length} samples`;
}
