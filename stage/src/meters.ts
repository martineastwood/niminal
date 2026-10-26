/** Anything quieter than this many dB is an empty meter. */
const FLOOR = -60;
/** How fast a bar falls, as a fraction of the meter per second. */
const FALL = 0.9;
/** How long the peak tick stays where it was, in seconds, before it falls. */
const HOLD = 1.2;

/** Where a linear peak sits on the meter: 0 at the floor, 1 at 0 dB. */
export function fraction(peak: number) {
  if (!(peak > 0)) return 0;
  return Math.min(1, Math.max(0, (20 * Math.log10(peak) - FLOOR) / -FLOOR));
}

/** One level meter: jumps up to a loud peak, falls back slowly, and keeps a tick at the highest recent peak. */
export class Bar {
  readonly element: HTMLElement;
  level = 0;
  hold = 0;
  /** A peak reached 0 dB or more; stays lit until it is clicked. */
  clipped = false;
  private heldFor = 0;
  private fill: HTMLElement;
  private tick: HTMLElement;

  constructor(className = '') {
    this.element = document.createElement('div');
    this.element.className = `meter ${className}`.trim();
    this.fill = document.createElement('i');
    this.fill.className = 'meter-fill';
    this.tick = document.createElement('i');
    this.tick.className = 'meter-hold';
    this.element.append(this.fill, this.tick);
    this.element.addEventListener('click', () => this.reset());
  }

  push(peak: number) {
    const f = fraction(peak);
    this.level = Math.max(this.level, f);
    if (f >= this.hold) {
      this.hold = f;
      this.heldFor = 0;
    }
    if (peak >= 1) this.clipped = true;
  }

  /** Let `seconds` pass. */
  step(seconds: number) {
    this.level = Math.max(0, this.level - FALL * seconds);
    this.heldFor += seconds;
    if (this.heldFor > HOLD) this.hold = Math.max(this.level, this.hold - FALL * seconds);
  }

  reset() {
    this.clipped = false;
    this.hold = this.level;
  }

  draw() {
    this.fill.style.transform = `scaleX(${this.level.toFixed(3)})`;
    this.tick.style.left = `${(this.hold * 100).toFixed(1)}%`;
    this.tick.style.opacity = this.hold > 0.01 ? '1' : '0';
    this.element.classList.toggle('hot', this.level > 0.92);
    this.element.classList.toggle('clipped', this.clipped);
  }
}

/** The loudest of the channels' peaks, for one bar. */
export function loudest(peaks: number[]) {
  return peaks.reduce((a, b) => Math.max(a, b), 0);
}
