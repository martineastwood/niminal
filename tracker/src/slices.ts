import { noteAt } from './project';

/** Where the pieces of a sliced sampler start, as shares of its sample (0 up to 1), read from its code. */
export interface Cuts { points: number[] }

const slicesCall = /(\.slices\()(\d+(?:\.\d+)?|\[[^\]]*\])(\))/;

/** The pieces `source` cuts its sample into: `.slices(16)` as sixteen equal ones, `.slices([0, 0.25])` as listed; null when it cuts none. */
export function readCuts(source: string): Cuts | null {
  const argument = slicesCall.exec(source)?.[2];
  if (argument === undefined) return null;
  if (!argument.startsWith('[')) {
    const count = Number(argument);
    return Number.isInteger(count) && count >= 1 ? { points: Array.from({ length: count }, (_, i) => i / count) } : null;
  }
  const points = argument.slice(1, -1).split(',').map(part => part.trim()).filter(Boolean).map(Number);
  return points.length && points.every(Number.isFinite) ? { points } : null;
}

/** `source` with its `.slices(...)` call changed to start pieces at `points`, and the count in its first comment kept right. */
export function writeCuts(source: string, points: number[]): string {
  const list = `[${points.map(share => Number(share.toFixed(5))).join(', ')}]`;
  return source.replace(slicesCall, (_, open, __, close) => `${open}${list}${close}`).replace(/^(\/\/\/ Plays .+? in )\d+( pieces)/m, `$1${points.length}$2`);
}

/** The smallest share of the sample between two cuts. */
export const minGap = 0.002;

/** `points` with cut `index` moved to `share`, kept between its neighbours. The first cut stays at 0. */
export function moveCut(points: number[], index: number, share: number): number[] {
  if (index <= 0 || index >= points.length) return points;
  const low = points[index - 1] + minGap, high = (points[index + 1] ?? 1) - minGap;
  const next = [...points]; next[index] = Math.min(high, Math.max(low, share));
  return next;
}

/** `points` with a cut added at `share`, or unchanged when it is too near another cut or the ends. */
export function addCut(points: number[], share: number): number[] {
  if (share <= minGap || share >= 1 - minGap || points.some(point => Math.abs(point - share) < minGap)) return points;
  return [...points, share].sort((a, b) => a - b);
}

/** `points` without cut `index`; the first cut, which starts the sample, and a lone cut stay. */
export function removeCut(points: number[], index: number): number[] {
  return index <= 0 || index >= points.length || points.length <= 2 ? points : points.filter((_, i) => i !== index);
}

/** Which piece plays at `share` of the sample: the last cut at or before it. */
export function pieceAt(points: number[], share: number): number {
  let piece = 0;
  points.forEach((point, index) => { if (point <= share) piece = index; });
  return piece;
}

/** The nearest cut to `share` within `reach`, other than the first, or -1. */
export function cutNear(points: number[], share: number, reach: number): number {
  let best = -1;
  points.forEach((point, index) => { if (index > 0 && Math.abs(point - share) <= reach && (best < 0 || Math.abs(point - share) < Math.abs(points[best] - share))) best = index; });
  return best;
}

/** The key that plays piece `index`: C-4 for the first. */
export const pieceKey = (index: number) => noteAt(48 + index) ?? '?';

/**
 * `source` of a sliced sampler written as one that plays its whole sample: the cut binding goes (its settings, such as
 * `.interp(sinc)`, move to the sample's), and the instrument plays the sample by pitch instead of by piece. Null when it is not cut.
 */
export function unslice(source: string): string | null {
  const cut = /^(\w+)\s*=\s*(\w+)\.slices\((?:\d+(?:\.\d+)?|\[[^\]]*\])\)([^\n]*?)(?=[ \t]*\/\/|$)[^\n]*\n/m.exec(source);
  if (!cut) return null;
  const [line, slices, base, settings] = cut;
  let text = source.replace(line, '');
  if (settings) text = text.replace(new RegExp(`^(${base}\\s*=\\s*sample\\("[^"\\n]+"\\)[^\\n]*?)(?=[ \\t]*\\/\\/|$)`, 'm'), (found: string) => `${found}${settings}`);
  text = text.replace(new RegExp(`\\b${slices}\\(([^)]*)\\)`, 'g'), (_, args: string) => {
    const kept = args.split(/,\s*/).filter(arg => arg.trim() && arg.trim() !== 'sample: sample');
    return `${base}(${['freq: freq', ...kept].join(', ')})`;
  });
  return text.replace(/sample: num = 0,\s*/, '').replace(/^\/\/\/ Plays (.+?) in \d+ pieces.*$/m, '/// Plays $1. Change .root(...) to retune, or add .loop(forward) to sustain it.');
}
