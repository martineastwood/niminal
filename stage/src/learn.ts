/**
 * Where `ctl <control> = midi.learn...` is in `doc`: the text to replace with what the control heard
 * (`midi.cc(21)`), or null if no line waits to learn that control.
 */
export function learnedRange(doc: string, control: string) {
  const escaped = control.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const line = new RegExp(`^[ \\t]*ctl[ \\t]+${escaped}\\b[^\\n]*?(midi\\.learn(?:\\(\\))?)`, 'm').exec(doc);
  if (!line) return null;
  const from = line.index + line[0].length - line[1].length;
  return { from, to: from + line[1].length };
}
