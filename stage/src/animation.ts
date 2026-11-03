import type { Snapshot } from './live';

/** Coast through a few missed polls, then freeze until the engine answers again. */
export function elapsedSince(updatedAt: number, now = performance.now()) {
  return Math.max(0, Math.min((now - updatedAt) / 1000, 0.25));
}

/** Display time only: every snapshot replaces this estimate, including clock resets and tempo changes. */
export function positionAt(snapshot: Snapshot, elapsed: number) {
  const t = snapshot.transport;
  if (!t) return null;
  const beats = snapshot.beats_per_bar || 4;
  const offset = t.beat - 1 + elapsed * t.bpm / 60;
  return { bar: t.bar + Math.floor(offset / beats), beat: 1 + offset % beats };
}
