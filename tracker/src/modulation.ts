import type { Parameter } from './project';

/** An LFO on one instrument parameter: it swings the parameter around its slider value, by `depth` of the parameter's range. */
export interface SourceLfo { shape: 'sine' | 'tri' | 'saw' | 'square'; rate: number; depth: number }
export const lfoShapes: SourceLfo['shape'][] = ['sine', 'tri', 'saw', 'square'];

// The tracker writes each LFO into the instrument's code as a comment that records its settings, then the line the comment describes:
//   // mod cutoff: tri 2hz 50%
//   cutoff = (cutoff + lfo(2hz, shape: tri) * 1900hz).clamp(200hz..4khz)
const marker = /^[ \t]*\/\/ mod (\w+): (sine|tri|saw|square) ([\d.]+)hz (\d+)%[ \t]*$/;

/** The units a modulated line can be written in. Decibel parameters are gains in the code, which a sum of decibels can't express. */
export const modulatable = (parameter: Parameter) => parameter.unit !== 'db';

const number = (value: number) => String(Number(value.toPrecision(6)));
const literal = (value: number, parameter: Parameter) => `${number(value)}${parameter.unit ?? ''}`;

function line(parameter: Parameter, lfo: SourceLfo) {
  const swing = lfo.depth * (parameter.max - parameter.min) / 2;
  const name = parameter.name;
  return `${name} = (${name} + lfo(${number(lfo.rate)}hz, shape: ${lfo.shape}) * ${literal(swing, parameter)}).clamp(${literal(parameter.min, parameter)}..${literal(parameter.max, parameter)})`;
}

/** The LFOs the code carries, by parameter. */
export function readLfos(source: string): Record<string, SourceLfo> {
  const found: Record<string, SourceLfo> = {};
  for (const text of source.split('\n')) {
    const match = marker.exec(text);
    if (match) found[match[1]] = { shape: match[2] as SourceLfo['shape'], rate: Number(match[3]), depth: Number(match[4]) / 100 };
  }
  return found;
}

/** Where the instrument's body begins: just after the `{` that follows its parameter list, or -1. */
function bodyStart(source: string) {
  const header = /\binstr\s+\w+\s*\(/.exec(source);
  if (!header) return -1;
  let depth = 1, at = header.index + header[0].length;
  for (; at < source.length && depth > 0; at++) depth += source[at] === '(' ? 1 : source[at] === ')' ? -1 : 0;
  const open = source.indexOf('{', at);
  return depth === 0 && open >= 0 ? open + 1 : -1;
}

/** The code with `parameter`'s LFO set to `lfo`, or taken out when `lfo` is null. Null if the code has no instrument to put it in. */
export function setLfo(source: string, parameter: Parameter, lfo: SourceLfo | null): string | null {
  const lines = source.split('\n');
  const at = lines.findIndex(text => marker.exec(text)?.[1] === parameter.name);
  if (at >= 0) lines.splice(at, 2);
  const rest = lines.join('\n');
  if (!lfo) return rest;
  const start = bodyStart(rest);
  if (start < 0) return null;
  const text = `// mod ${parameter.name}: ${lfo.shape} ${number(lfo.rate)}hz ${Math.round(lfo.depth * 100)}%`;
  return `${rest.slice(0, start)}\n  ${text}\n  ${line(parameter, lfo)}${rest.slice(start).startsWith('\n') ? '' : '\n'}${rest.slice(start)}`;
}
