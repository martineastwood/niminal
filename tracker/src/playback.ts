import { readLoop, writeLoop } from './loops';

/**
 * What a sampler does when its note ends: a one-shot plays to its end whatever the note does (`gate` 0), the others
 * follow it (`gate` 1) and either stop with it (hold) or repeat a region of the sample until it ends (loop, ping-pong).
 */
export type Playback = 'one-shot' | 'hold' | 'loop' | 'ping-pong';

export const playbackNames: Record<Playback, string> = { 'one-shot': 'One-shot', hold: 'Hold', loop: 'Loop', 'ping-pong': 'Ping-pong' };
export const playbackHints: Record<Playback, string> = {
  'one-shot': 'Plays to its end, whatever the note does: OFF and CUT are ignored',
  hold: 'Plays while the note is held, and ends with it',
  loop: 'Repeats the marked region while the note is held',
  'ping-pong': 'Repeats the marked region, back and forth, while the note is held',
};

const gate = /gate: 0\.\.1 = ([\d.]+)/;

/** How a sampler (or kit) of the project plays, or null when its code is not one of the tracker's. */
export function readPlayback(source: string): Playback | null {
  if (!gate.test(source) || !/\b(?:sample|kit)\("/.test(source)) return null;
  const { mode } = readLoop(source);
  return mode === 'forward' ? 'loop' : mode === 'bidirectional' ? 'ping-pong' : Number(gate.exec(source)![1]) >= 0.5 ? 'hold' : 'one-shot';
}

/** The code of a sampler changed to play as `playback`; unchanged when it is not one of the tracker's, or when it cannot loop (a cut or kit sample). */
export function writePlayback(source: string, playback: Playback): string {
  if (readPlayback(source) === null) return source;
  const loop = readLoop(source);
  if (playback === 'loop' || playback === 'ping-pong') {
    const looped = writeLoop(source, { ...loop, mode: playback === 'loop' ? 'forward' : 'bidirectional' });
    return looped === source ? source : looped.replace(gate, 'gate: 0..1 = 1');
  }
  const flat = loop.mode === 'none' ? source : writeLoop(source, { ...loop, mode: 'none' });
  return flat.replace(gate, `gate: 0..1 = ${playback === 'hold' ? 1 : 0}`);
}

/** What an imported file most likely is, from its name and length: a drum hit, a loop or pad that is held, or by its length. */
export function guessPlayback(name: string, seconds: number, longSample: number): Playback {
  if (/loop|pad|drone|ambien|atmos|texture/i.test(name)) return 'loop';
  if (/kick|snare|hat|clap|hit|shot|perc|tom|rim|stab|crash|cymbal/i.test(name)) return 'one-shot';
  return seconds >= longSample ? 'hold' : 'one-shot';
}
