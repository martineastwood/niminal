// What a performer can touch: the clips of the running piece and its controls (sliders). This is the
// description of a piece that the Studio's Perform panel draws, and the commands it sends back. Nothing here knows
// about the page, so the same description can be drawn on another device.
import type { Status } from './engine';

/** How far a slider goes, in the units the engine takes: hertz, decibels, or a plain number. */
export type Range = { min: number; max: number; unit: 'hz' | 'db' | ''; log: boolean };

export type ControlSpec = {
  name: string;
  /** the name as shown */
  label: string;
  range: Range;
  /** where the control is now, in the range's own units (decibels, not gain) */
  value: number;
};

export type ClipSpec = { name: string; running: boolean; muted: boolean };

export type PerformState = {
  transport: { bar: number; beat: number; bpm: number } | null;
  clips: ClipSpec[];
  controls: ControlSpec[];
};

export type PerformCommand =
  | { kind: 'control'; name: string; value: number; unit: Range['unit'] }
  | { kind: 'clip'; name: string; action: 'start' | 'stop'; when?: string };

/** The code that carries out a clip command; controls are not code, they move directly. */
export const clipCode = (c: Extract<PerformCommand, { kind: 'clip' }>) => `${c.name}.${c.action}`;

// ---- ranges: said in a comment on the `ctl` line, or guessed from the control's unit and starting value

const UNITS: Record<string, { scale: number; unit: Range['unit'] }> = {
  hz: { scale: 1, unit: 'hz' },
  khz: { scale: 1000, unit: 'hz' },
  db: { scale: 1, unit: 'db' },
  '': { scale: 1, unit: '' },
};

function quantity(text: string): { value: number; unit: Range['unit'] } | null {
  const m = /^(-?\d*\.?\d+)\s*([a-z]*)$/i.exec(text.trim());
  const u = m && UNITS[m[2].toLowerCase()];
  return m && u ? { value: Number(m[1]) * u.scale, unit: u.unit } : null;
}

/** `ctl cutoff = 1khz  // range 300hz..12khz` says that the slider for `cutoff` goes from 300 Hz to 12 kHz. */
export function annotatedRanges(source: string): Map<string, Range> {
  const found = new Map<string, Range>();
  for (const line of source.split('\n')) {
    const m = /^\s*ctl\s+([A-Za-z_]\w*)\b.*?\/\/\s*range\s+(\S+)\s*\.\.\s*(\S+)/.exec(line);
    if (!m) continue;
    const lo = quantity(m[2]);
    const hi = quantity(m[3]);
    if (!lo || !hi || !(hi.value > lo.value)) continue;
    // frequencies are felt in ratios, so they slide on a log scale
    found.set(m[1], { min: lo.value, max: hi.value, unit: lo.unit, log: lo.unit === 'hz' && lo.value > 0 });
  }
  return found;
}

/** A range when the piece does not say: from the unit and where the control starts. */
function guessRange(unit: string | null, value: number): Range {
  if (unit === 'hz') return { min: Math.max(20, value / 8), max: Math.min(20000, Math.max(value * 4, 100)), unit: 'hz', log: true };
  if (unit === 'db') return { min: -60, max: 6, unit: 'db', log: false };
  if (value >= 0 && value <= 1) return { min: 0, max: 1, unit: '', log: false };
  return value >= 0 ? { min: 0, max: Math.max(1, value * 2), unit: '', log: false } : { min: value * 2, max: -value * 2, unit: '', log: false };
}

const asDecibels = (gain: number) => 20 * Math.log10(Math.max(gain, 1e-6));

/** The Perform description of a running piece, from the engine's status and the ranges its source declares. */
export function describe(status: Status['status'], ranges: Map<string, Range>): PerformState {
  if (!status) return { transport: null, clips: [], controls: [] };
  return {
    transport: { bar: status.bar, beat: status.beat, bpm: status.bpm },
    clips: status.clips.map((c) => ({ name: c.name, running: c.running, muted: c.muted })),
    controls: status.controls.map((c) => {
      const range = ranges.get(c.name) ?? guessRange(c.unit, c.unit === 'db' ? asDecibels(c.value) : c.value);
      // the engine reports a decibel control as a gain; a slider speaks decibels
      const value = range.unit === 'db' ? asDecibels(c.value) : c.value;
      return { name: c.name, label: c.name.replace(/_/g, ' '), range, value };
    }),
  };
}

// ---- a slider's position (0..1) and the value it stands for
export const valueAt = (r: Range, t: number) => (r.log ? r.min * (r.max / r.min) ** t : r.min + (r.max - r.min) * t);
export const positionOf = (r: Range, v: number) =>
  Math.min(1, Math.max(0, r.log ? Math.log(Math.max(v, r.min) / r.min) / Math.log(r.max / r.min) : (v - r.min) / (r.max - r.min)));

export function format(r: Range, v: number) {
  if (r.unit === 'hz') return v >= 1000 ? `${(v / 1000).toFixed(1)} kHz` : `${Math.round(v)} Hz`;
  if (r.unit === 'db') return `${v.toFixed(0)} dB`;
  return v.toFixed(2);
}
