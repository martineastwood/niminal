import { invoke } from '@tauri-apps/api/core';
import { Gallery } from './gallery';

/** An instrument parameter as the engine reports it: db as linear gain, times in seconds. */
export type LabParam = { name: string; unit: string; default: number | null; range: [number, number] | null };
export type LabInstrument = { name: string; params: LabParam[] };

/** What a fader spans, in the unit its value is sent in (db in db, times in seconds). */
export type Fader = { name: string; unit: string; min: number; max: number; start: number; log: boolean };

/** Parameters a keyboard plays or a performer moves, not something to set by hand. */
const played = new Set(['freq', 'gain', 'bend', 'pressure', 'slide']);

/** The fader for a parameter, or null when it has no range to span. */
export function faderFor(param: LabParam): Fader | null {
  if (!param.range || played.has(param.name)) return null;
  const db = param.unit === 'db';
  const [lo, hi] = db ? param.range.map(v => 20 * Math.log10(v)) : param.range;
  if (!(hi > lo)) return null;
  const given = param.default === null ? null : db ? 20 * Math.log10(param.default) : param.default;
  const start = Math.min(hi, Math.max(lo, given ?? (lo + hi) / 2));
  const log = (param.unit === 'hz' || param.unit === 'sec') && lo > 0;
  return { name: param.name, unit: param.unit, min: lo, max: hi, start, log };
}

/** The value at `position` (0 to 1) along a fader. */
export function valueAt(fader: Fader, position: number) {
  const t = Math.min(1, Math.max(0, position));
  if (fader.log) return fader.min * (fader.max / fader.min) ** t;
  return fader.min + (fader.max - fader.min) * t;
}

/** Where `value` sits along a fader, 0 to 1. */
export function positionOf(fader: Fader, value: number) {
  const t = fader.log
    ? Math.log(value / fader.min) / Math.log(fader.max / fader.min)
    : (value - fader.min) / (fader.max - fader.min);
  return Math.min(1, Math.max(0, t));
}

/** A value the way it is written in code: `250ms`, `1.2khz`, `-6db`. */
export function format(unit: string, value: number) {
  switch (unit) {
    case 'hz':
      return value >= 1000 ? `${(value / 1000).toFixed(2)}khz` : `${value.toFixed(value < 10 ? 2 : 1)}hz`;
    case 'sec':
      return value < 1 ? `${Math.round(value * 1000)}ms` : `${value.toFixed(2)}sec`;
    case 'db':
      return `${value.toFixed(1)}db`;
    case 'deg':
      return `${Math.round(value)}deg`;
    case 'st':
      return `${value.toFixed(1)}st`;
    default:
      return value.toFixed(2);
  }
}

/** The computer keyboard as one octave of piano keys, as semitones above the octave's first key. */
export const pianoKeys: Record<string, number> = {
  a: 0, w: 1, s: 2, e: 3, d: 4, f: 5, t: 6, g: 7, y: 8, h: 9, u: 10, j: 11, k: 12,
};

/** The clip the Lab plays an instrument through: any keys held sound it. */
export const auditionClip = (instrument: string) =>
  `clip __lab {\n  instrument: ${instrument}\n  keys: midi.keys(channel: 1)\n}\n__lab.start`;

type Options = {
  evaluate: (source: string) => Promise<void>;
  openPiece: (path: string) => void;
  status: (message: string) => void;
};

/** The Lab: pick an instrument, hold notes on it, and move its parameters while it sounds. */
export class Lab {
  readonly element: HTMLElement;
  private select: HTMLSelectElement;
  private faders: HTMLElement;
  private octave = 4;
  private down = new Set<number>();
  private instruments: LabInstrument[] = [];
  private loaded = '';
  private gallery: Gallery;
  private view: 'play' | 'gallery' = 'play';

  constructor(private options: Options) {
    this.element = document.createElement('aside');
    this.element.id = 'lab';
    this.element.innerHTML = `
      <div class="lab-tabs"><button class="lab-tab on" data-view="play" title="Play one instrument and move its parameters">Play</button><button class="lab-tab" data-view="gallery" title="Make many versions of a sample and hear them">Gallery</button></div>
      <div class="lab-play">
      <div class="lab-head">
        <strong>lab</strong>
        <select class="lab-instrument" title="The instrument to play"></select>
        <button class="lab-refresh" title="Read the instruments again, after evaluating new ones">Refresh</button>
      </div>
      <div class="lab-keys" title="Hold a key to play. The computer keys A W S E D F T G Y H U J K work too, and Z / X move the octave"></div>
      <div class="lab-octave"><button class="lab-down" title="Octave down (Z)">−</button><span></span><button class="lab-up" title="Octave up (X)">+</button><button class="lab-hold" title="Keep a note sounding while you move the faders">Hold</button></div>
      <div class="lab-faders"></div>
      </div>`;
    const find = <T extends HTMLElement>(selector: string) => this.element.querySelector<T>(selector)!;
    this.select = find('.lab-instrument');
    this.faders = find('.lab-faders');
    this.gallery = new Gallery({ openPiece: options.openPiece, status: options.status });
    this.gallery.element.hidden = true;
    this.element.append(this.gallery.element);
    for (const tab of this.element.querySelectorAll<HTMLElement>('.lab-tab')) {
      tab.addEventListener('click', () => this.switchTo(tab.dataset.view as 'play' | 'gallery'));
    }
    this.select.addEventListener('change', () => void this.choose());
    find('.lab-refresh').addEventListener('click', () => void this.refresh());
    find('.lab-down').addEventListener('click', () => this.shift(-1));
    find('.lab-up').addEventListener('click', () => this.shift(1));
    const hold = find('.lab-hold');
    hold.addEventListener('click', () => {
      const on = hold.classList.toggle('on');
      this.key(this.root() - 12, on);
    });
    const keys = find('.lab-keys');
    for (let semitone = 0; semitone <= 12; semitone++) {
      const button = document.createElement('button');
      button.className = [1, 3, 6, 8, 10].includes(semitone) ? 'black' : 'white';
      button.addEventListener('pointerdown', () => this.key(this.root() + semitone, true));
      for (const end of ['pointerup', 'pointerleave', 'pointercancel']) {
        button.addEventListener(end, () => this.key(this.root() + semitone, false));
      }
      keys.append(button);
    }
    this.shift(0);
    window.addEventListener('keydown', event => this.typed(event, true));
    window.addEventListener('keyup', event => this.typed(event, false));
  }

  get open() {
    return document.body.classList.contains('lab');
  }

  async toggle() {
    document.body.classList.toggle('lab', !this.open);
    if (!this.open) {
      this.releaseAll();
      this.gallery.stop();
    }
    else if (this.view === 'play') await this.refresh();
  }

  private switchTo(view: 'play' | 'gallery') {
    this.view = view;
    this.releaseAll();
    this.gallery.stop();
    this.element.querySelector<HTMLElement>('.lab-play')!.hidden = view !== 'play';
    this.gallery.element.hidden = view !== 'gallery';
    for (const tab of this.element.querySelectorAll<HTMLElement>('.lab-tab')) {
      tab.classList.toggle('on', tab.dataset.view === view);
    }
  }

  /** The MIDI note of the keyboard's first key: c in the current octave. */
  private root() {
    return 12 * (this.octave + 1);
  }

  private shift(by: number) {
    this.releaseAll();
    this.octave = Math.min(8, Math.max(0, this.octave + by));
    this.element.querySelector('.lab-octave span')!.textContent = `c${this.octave}`;
  }

  private typed(event: KeyboardEvent, on: boolean) {
    const target = event.target as HTMLElement | null;
    if (!this.open || this.view !== 'play' || event.repeat || event.metaKey || event.ctrlKey || event.altKey) return;
    if (target && (target.closest('.cm-editor') || /^(input|select|textarea)$/i.test(target.tagName))) return;
    const key = event.key.toLowerCase();
    if (key in pianoKeys) this.key(this.root() + pianoKeys[key], on);
    else if (on && key === 'z') this.shift(-1);
    else if (on && key === 'x') this.shift(1);
  }

  private key(note: number, on: boolean) {
    if (on === this.down.has(note)) return;
    if (on) this.down.add(note);
    else this.down.delete(note);
    invoke<number>('lab_key', { note, velocity: on ? 100 : 0 }).catch(e => this.options.status(String(e)));
  }

  private releaseAll() {
    for (const note of [...this.down]) this.key(note, false);
    this.element.querySelector('.lab-hold')?.classList.remove('on');
  }

  /** Read the instruments the engine has, keeping the one that was chosen. */
  async refresh() {
    try {
      this.instruments = (await invoke<LabInstrument[]>('lab_instruments')).filter(
        i => !i.name.startsWith('__') && i.params.some(p => p.name === 'freq'),
      );
    } catch (error) {
      this.options.status(String(error));
      return;
    }
    const chosen = this.select.value;
    this.select.replaceChildren(
      ...this.instruments.map(i => {
        const option = document.createElement('option');
        option.value = option.textContent = i.name;
        return option;
      }),
    );
    if (this.instruments.some(i => i.name === chosen)) this.select.value = chosen;
    this.loaded = '';
    await this.choose();
  }

  /** Play through the chosen instrument and draw a fader for each parameter with a range. */
  private async choose() {
    this.releaseAll();
    const instrument = this.instruments.find(i => i.name === this.select.value);
    this.faders.replaceChildren();
    if (!instrument) {
      this.faders.textContent = 'No instrument with a freq parameter. Evaluate one, then press Refresh.';
      return;
    }
    if (this.loaded !== instrument.name) {
      this.loaded = instrument.name;
      await this.options.evaluate(auditionClip(instrument.name));
    }
    for (const fader of instrument.params.map(faderFor).filter((f): f is Fader => f !== null)) {
      this.faders.append(this.row(fader));
    }
  }

  private row(fader: Fader) {
    const row = document.createElement('label');
    row.className = 'lab-fader';
    const name = document.createElement('span');
    name.textContent = fader.name;
    const slider = document.createElement('input');
    slider.type = 'range';
    slider.min = '0';
    slider.max = '1000';
    const readout = document.createElement('output');
    const move = (value: number) => {
      readout.textContent = format(fader.unit, value);
      invoke<number>('lab_param', { name: fader.name, value }).catch(e => this.options.status(String(e)));
    };
    slider.value = String(Math.round(positionOf(fader, fader.start) * 1000));
    slider.addEventListener('input', () => move(valueAt(fader, Number(slider.value) / 1000)));
    slider.addEventListener('dblclick', () => {
      slider.value = String(Math.round(positionOf(fader, fader.start) * 1000));
      move(fader.start);
    });
    move(fader.start);
    row.append(name, slider, readout);
    return row;
  }
}
