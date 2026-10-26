import { describe, expect, it } from 'vitest';
import { HealthWatch } from './health';

const reading = (starved: number) => ({ late_events: 0, starved_frames: starved, deadline_misses: 0, capacity_drops: 0, max_render_micros: 0 });

describe('watching the engine', () => {
  it('is fine while the counters stand still', () => {
    const watch = new HealthWatch();
    watch.update(reading(5), 0);
    expect(watch.update(reading(5), 1000).ok).toBe(true);
  });

  it('is in trouble for a few seconds after a counter rises, and says what rose', () => {
    const watch = new HealthWatch();
    watch.update(reading(5), 0);
    const trouble = watch.update(reading(8), 1000);
    expect(trouble.ok).toBe(false);
    expect(trouble.detail).toBe('3 starved frames');
    expect(watch.update(reading(8), 5000).ok).toBe(false);
    expect(watch.update(reading(8), 10000).ok).toBe(true);
  });

  it('does not count the engine restarting as trouble', () => {
    const watch = new HealthWatch();
    watch.update(reading(50), 0);
    expect(watch.update(reading(0), 1000).ok).toBe(true);
  });

  it('says so when the engine does not report', () => {
    expect(new HealthWatch().update(null, 0).known).toBe(false);
  });
});
