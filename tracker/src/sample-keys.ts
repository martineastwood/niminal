import { noteAt } from './project';

const first = 36, last = 71, black = new Set([1, 3, 6, 8, 10]);

/** A strip of keys, C-3 to B-5, for hearing the sample at any pitch: a key sounds while it is held, and the ones that play a piece of a cut sample are marked. */
export class SampleKeys {
  readonly root = document.createElement('div');
  private readonly keys = new Map<number, HTMLButtonElement>();
  private down: number | null = null;

  constructor(private readonly play: (note: string, key: number) => void, private readonly stop: (key: number) => void) {
    this.root.className = 'sample-keys'; this.root.setAttribute('aria-label', 'Sample keyboard');
    const whites = (last - first + 1) * 7 / 12;
    let white = 0;
    for (let number = first; number <= last; number++) {
      const key = document.createElement('button'), note = noteAt(number)!;
      key.className = black.has(number % 12) ? 'key black' : 'key white'; key.title = note; key.setAttribute('aria-label', note);
      if (number % 12 === 0) key.textContent = note;
      if (black.has(number % 12)) { key.style.left = `${(white / whites) * 100}%`; key.style.width = `${(0.6 / whites) * 100}%`; key.style.transform = 'translateX(-50%)'; } else white++;
      key.onpointerdown = event => { event.preventDefault(); this.release(); this.down = number; this.play(note, number); };
      key.onpointerenter = event => { if (event.buttons && this.down !== null && this.down !== number) { this.release(); this.down = number; this.play(note, number); } };
      this.keys.set(number, key); this.root.append(key);
    }
    this.root.onpointerup = this.root.onpointerleave = () => this.release();
  }

  private release() { if (this.down !== null) { this.stop(this.down); this.down = null; } }

  /** Marks the `count` keys from C-4 that play pieces, none when it is not cut. */
  mark(count: number) {
    for (const [number, key] of this.keys) key.classList.toggle('mapped', number >= 48 && number < 48 + count);
  }
}
