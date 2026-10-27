import { columnStrip, type EffectParameters, type FxTarget, type Parameter, type Envelope, type Pattern, type Project } from './project';
import { instrumentsOnTrack } from './instruments';

export interface LaneParameter extends Parameter { label: string }
/** What a lane on `track` can drive: its volume and pan, its effects' parameters, and those of the instruments it plays (and any a lane already points at). */
export function laneChoices(project: Project, pattern: Pattern, track: number, parameters: EffectParameters): { target: FxTarget; owner: string; name: string }[] {
  const strip = columnStrip(project, track);
  // only a track has notes, so only a track has instruments to automate
  const listed = new Set(track < project.tracks.length ? [...instrumentsOnTrack(project, track), ...pattern.fx[track].flatMap(lane => lane.target?.kind === 'instrument' ? [lane.target.instrument] : [])] : []);
  const choices: { target: FxTarget; owner: string; name: string }[] = [{ target: { kind: 'volume' }, owner: 'Track', name: 'Vol' }, { target: { kind: 'pan' }, owner: 'Track', name: 'Pan' }];
  for (const definition of project.definitions) if (definition.kind === 'instr' && listed.has(definition.id)) for (const p of parameters[definition.id] ?? []) choices.push({ target: { kind: 'instrument', instrument: definition.id, parameter: p.name }, owner: definition.name, name: p.name });
  strip.macros?.forEach((knob, index) => choices.push({ target: { kind: 'macro', index }, owner: 'Macros', name: knob.name || `Macro ${index + 1}` }));
  const driven = (device: string, parameter: string) => strip.macros?.some(knob => knob.links.some(link => link.device === device && link.parameter === parameter));
  for (const device of strip.fx) for (const p of (parameters[device.definition] ?? []).filter(p => !driven(device.id, p.name))) choices.push({ target: { kind: 'effect', device: device.id, parameter: p.name }, owner: project.definitions.find(d => d.id === device.definition)?.name ?? 'Effect', name: p.name });
  return choices;
}
export function targetKey(target: FxTarget | null) { return target ? JSON.stringify(target) : ''; }
export function laneParameter(target: FxTarget | null, track: number, project: Project, parameters: EffectParameters): LaneParameter | null {
  if (!target) return null;
  if (target.kind === 'volume') return { label: 'Track volume', name: 'volume', min: -60, max: 6, default: 0, unit: 'db' };
  if (target.kind === 'pan') return { label: 'Track pan', name: 'pan', min: -1, max: 1, default: 0, unit: '%' };
  if (target.kind === 'macro') {
    const knob = columnStrip(project, track).macros?.[target.index];
    return knob ? { label: `Macro → ${knob.name || target.index + 1}`, name: 'macro', min: 0, max: 1, default: knob.value, unit: null } : null;
  }
  if (target.kind === 'instrument') {
    const parameter = parameters[target.instrument]?.find(p => p.name === target.parameter);
    if (!parameter) return null;
    return { ...parameter, label: `${project.definitions.find(d => d.id === target.instrument)?.name ?? 'Instrument'} → ${parameter.name}` };
  }
  const device = columnStrip(project, track).fx.find(d => d.id === target.device);
  const parameter = parameters[device?.definition ?? '']?.find(p => p.name === target.parameter);
  if (!device || !parameter) return null;
  const name = project.definitions.find(d => d.id === device.definition)?.name ?? 'Effect';
  return { ...parameter, label: `${name} → ${parameter.name}${device.bypass ? ' (bypassed)' : ''}` };
}
function percent(p: Parameter) { return p.unit === '%' || !p.unit && p.min === 0 && p.max === 1; }
/** Rounds for display; the stored value keeps its precision. `exact` keeps six significant figures, for the clipboard. */
/** `bare` leaves the unit off a dB value, for a cell in a lane whose parameter already says what it is. */
export function formatValue(value: string | number, p: Parameter | null, exact = false, bare = false): string {
  const n = Number(value);
  const round = (x: number, places: number) => String(exact ? Number(x.toPrecision(6)) : Number(x.toFixed(places)));
  if (!p) return round(n, 2);
  if (percent(p)) return `${round(n * 100, 0)}%`;
  if (p.unit === 'hz') return n >= 1000 ? `${round(n / 1000, 2)} kHz` : `${round(n, 0)} Hz`;
  if (p.unit === 'sec') return n >= 0.1 || exact ? `${round(n * 1000, 0)} ms` : `${round(n * 1000, 1)} ms`;
  return `${round(n, p.unit === 'db' ? 1 : 2)}${p.unit && !(bare && p.unit === 'db') ? ' ' + p.unit : ''}`;
}
export function parseValue(text: string, p: Parameter): string | null {
  const match = text.trim().match(/^([-+]?\d*\.?\d+(?:e[-+]?\d+)?)\s*(khz|hz|ms|sec|s|db|%|st|deg)?$/i);
  if (!match) return null;
  let n = Number(match[1]); const unit = match[2]?.toLowerCase();
  if (unit === '%') { if (!percent(p)) return null; n /= 100; }
  else if (unit === 'khz' || unit === 'hz') { if (p.unit !== 'hz') return null; if (unit === 'khz') n *= 1000; }
  else if (unit === 'ms' || unit === 'sec' || unit === 's') { if (p.unit !== 'sec') return null; if (unit === 'ms') n /= 1000; }
  else if (unit) { if (unit !== p.unit) return null; }
  else if (percent(p)) n /= 100;
  else if (p.unit === 'sec') n /= 1000;
  return Number.isFinite(n) && n >= p.min - 1e-9 && n <= p.max + 1e-9 ? String(Math.min(p.max, Math.max(p.min, n))) : null;
}

// Native glides interpolate engine units; dB controls are linear in gain.
export function envelopeValue(envelope: Envelope, row: number, db = false): number | null {
  const before = [...envelope.points].reverse().find(p => p.row <= row);
  if (!before) return null;
  const after = envelope.points.find(p => p.row > row);
  if (envelope.mode === 'hold' || !after) return before.value;
  const fraction = (row - before.row) / (after.row - before.row);
  if (db) return 20 * Math.log10(10 ** (before.value / 20) * (1 - fraction) + 10 ** (after.value / 20) * fraction);
  return before.value + (after.value - before.value) * fraction;
}
