import { binding } from './loops';

/** The properties of a sampler that its code sets and the Sample view shows: its root pitch, release, and interpolation. */

const names = ['C-', 'C#', 'D-', 'D#', 'E-', 'F-', 'F#', 'G-', 'G#', 'A-', 'A#', 'B-'];
/** A note number (60 is C-4) as the tracker writes it. */
export const noteName = (midi: number) => `${names[((midi % 12) + 12) % 12]}${Math.floor(midi / 12) - 1}`;

/** The note a sample plays as recorded, and how many cents (-50 to 50) above it. */
export interface Root { midi: number; cents: number }

const rootCall = /\.root\(\s*([^)]*?)\s*\)/;
const letters: Record<string, number> = { c: 0, d: 2, e: 4, f: 5, g: 7, a: 9, b: 11 };
const hz = (midi: number) => 440 * 2 ** ((midi - 69) / 12);

/** The root pitch of the sampler's sample, from a note (`c4`) or a frequency (`261.6hz`); null when it is written some other way. */
export function readRoot(source: string): Root | null {
  const text = rootCall.exec(binding.exec(source)?.[0] ?? '')?.[1];
  if (text === undefined) return null;
  const note = /^([a-g])(#|s|b)?(-?\d)$/i.exec(text), frequency = /^([\d.]+)\s*hz$/i.exec(text);
  let exact: number;
  if (note) exact = (Number(note[3]) + 1) * 12 + letters[note[1].toLowerCase()] + (note[2] === 'b' ? -1 : note[2] ? 1 : 0);
  else if (frequency && Number(frequency[1]) > 0) exact = 69 + 12 * Math.log2(Number(frequency[1]) / 440);
  else return null;
  const midi = Math.round(exact);
  return { midi, cents: Math.round((exact - midi) * 100) };
}

/** `source` with its sample's root pitch set; a plain note is written as one, and anything with cents as a frequency. */
export function writeRoot(source: string, root: Root): string {
  if (readRoot(source) === null) return source;
  const natural = Object.entries(letters).find(([, offset]) => offset === ((root.midi % 12) + 12) % 12);
  const text = root.cents === 0 && natural ? `${natural[0]}${Math.floor(root.midi / 12) - 1}` : `${Number(hz(root.midi + root.cents / 100).toFixed(3))}hz`;
  return source.replace(binding, line => line.replace(rootCall, `.root(${text})`));
}

/** How long a sampler takes to fade out when its note ends, in ms; null when it has no `release`. */
export function readRelease(source: string): number | null {
  const found = /release: 0ms\.\.2sec = ([\d.]+)ms/.exec(source)?.[1];
  return found === undefined ? null : Number(found);
}

export const writeRelease = (source: string, ms: number) => source.replace(/release: 0ms\.\.2sec = [\d.]+ms/, `release: 0ms..2sec = ${Number(ms.toFixed(1))}ms`);

export type Interpolation = 'linear' | 'cubic' | 'sinc';

/** The binding the instrument plays through, which is where its playback settings go: the cut sample of a sliced instrument, else the sample. */
const playing = /^\w+\s*=\s*(?:sample\("[^"\n]+"\)|\w+\.slices\()[^\n]*?(?=[ \t]*\/\/|$)/gm;
const lastBinding = (source: string) => [...source.matchAll(playing)].at(-1);
const interpCall = /\.interp\(\s*(linear|cubic|sinc)\s*\)/;

/** How the sampler reads between its sample's frames; null when its code has no sample to set it on. */
export function readInterpolation(source: string): Interpolation | null {
  const line = lastBinding(source)?.[0];
  return line === undefined ? null : (interpCall.exec(line)?.[1] as Interpolation | undefined) ?? 'cubic';
}

/** `source` with its interpolation set: linear is the cheapest and roughest, sinc the cleanest at high pitches and the dearest, and cubic is the default, which is left unwritten. */
export function writeInterpolation(source: string, interpolation: Interpolation): string {
  const found = lastBinding(source);
  if (!found) return source;
  const call = interpolation === 'cubic' ? '' : `.interp(${interpolation})`;
  const line = interpCall.test(found[0]) ? found[0].replace(interpCall, call) : `${found[0]}${call}`;
  return source.slice(0, found.index) + line + source.slice(found.index + found[0].length);
}
