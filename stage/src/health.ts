import { Snapshot } from './live';

type Health = NonNullable<Snapshot['health']>;

/** How long the engine counts as in trouble after the last dropout, in milliseconds. */
const TROUBLE_FOR = 8000;

const LABELS: [keyof Health, string][] = [
  ['starved_frames', 'starved frames'],
  ['deadline_misses', 'late blocks'],
  ['late_events', 'late notes'],
  ['capacity_drops', 'dropped voices'],
];

/**
 * Watches the engine's counters for the things that make audio glitch. They only go up, so a glitch is a
 * rise since the last reading; the engine counts as in trouble for a few seconds after one.
 */
export class HealthWatch {
  private last: Health | null = null;
  private until = 0;
  private what: string[] = [];

  /** Take a reading; returns what is wrong now (empty when the engine is fine) and when it was last wrong. */
  update(health: Health | null | undefined, now: number) {
    if (health && this.last) {
      const rose = LABELS.filter(([key]) => health[key] > this.last![key]).map(
        ([key, label]) => `${health[key] - this.last![key]} ${label}`,
      );
      if (rose.length) {
        this.until = now + TROUBLE_FOR;
        this.what = rose;
      }
    }
    this.last = health ?? null;
    return { ok: now >= this.until, detail: this.what.join(', '), known: health != null };
  }
}
