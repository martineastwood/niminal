import { Arrangement } from './live';

/**
 * What pressing an arrangement sends. Playing, it stops: the lines that have not come yet are dropped
 * (`song.stop`) and what is sounding is let go (`hush`). Otherwise it starts, on the land box's boundary.
 */
export function arrangementCommand(arrangement: Arrangement) {
  return arrangement.running ? `${arrangement.name}.stop\nhush` : `${arrangement.name}.start`;
}

/** How far through it is, from 0 to 1; 0 when it is not running. */
export function arrangementProgress(arrangement: Arrangement) {
  if (!arrangement.running || arrangement.elapsed === null || arrangement.length <= 0) return 0;
  return Math.min(1, Math.max(0, arrangement.elapsed / arrangement.length));
}

/** Seconds as `m:ss`. */
export function clock(seconds: number) {
  const whole = Math.max(0, Math.round(seconds));
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, '0')}`;
}

/** The words beside the name: the section while it plays, otherwise how long it is. */
export function arrangementLabel(arrangement: Arrangement) {
  if (!arrangement.running) return arrangement.length > 0 ? clock(arrangement.length) : '';
  const where = clock(arrangement.elapsed ?? 0);
  return arrangement.section ? `${arrangement.section} · ${where}` : where;
}
