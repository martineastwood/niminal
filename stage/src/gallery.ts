import { invoke } from '@tauri-apps/api/core';

/** One version a gallery made, as `gallery.json` lists it. */
export type Tile = { name: string; recipe: string; family: string; values: Record<string, number> };
type Made = { dir: string; tiles: Tile[] };

export const families = ['players', 'manglers', 'glitch', 'synthesis', 'modifiers', 'replicators'];

/** The settings a version was made from, shortest first: `density 15.1 · grain 0.25`. */
export function summary(values: Record<string, number>, count = 3) {
  return Object.entries(values)
    .slice(0, count)
    .map(([name, value]) => `${name} ${Number(value.toPrecision(3))}`)
    .join(' · ');
}

type Options = { openPiece: (path: string) => void; status: (message: string) => void };

/** The gallery: pick a sample, make many versions of it as code, hear each and open the ones you like. */
export class Gallery {
  readonly element: HTMLElement;
  private sample = localStorage.getItem('niminal-stage:gallery-sample');
  private seed = 1;
  private made: Made | null = null;
  private busy = false;
  private context: AudioContext | null = null;
  private playing: { source: AudioBufferSourceNode; name: string } | null = null;
  private where: HTMLElement;
  private grid: HTMLElement;
  private make: HTMLButtonElement;
  private amount: HTMLSelectElement;
  private checks: HTMLInputElement[];

  constructor(private options: Options) {
    this.element = document.createElement('div');
    this.element.className = 'gallery';
    this.element.innerHTML = `
      <div class="gallery-head">
        <button class="gallery-pick" title="Choose the sample to make versions of">Sample…</button>
        <span class="gallery-where"></span>
      </div>
      <div class="gallery-families">${families
        .map(f => `<label><input type="checkbox" value="${f}" checked /> ${f}</label>`)
        .join('')}</div>
      <div class="gallery-head">
        <select class="gallery-amount" title="How many versions of each recipe">
          <option value="few">few</option><option value="enough" selected>enough</option><option value="lots">lots</option>
        </select>
        <button class="gallery-make primary" title="Make versions. Press again for a new set">Make</button>
      </div>
      <div class="gallery-grid" role="list"></div>`;
    const find = <T extends HTMLElement>(selector: string) => this.element.querySelector<T>(selector)!;
    this.where = find('.gallery-where');
    this.grid = find('.gallery-grid');
    this.make = find('.gallery-make');
    this.amount = find('.gallery-amount');
    this.checks = [...this.element.querySelectorAll<HTMLInputElement>('.gallery-families input')];
    find('.gallery-pick').addEventListener('click', () => void this.pick());
    this.make.addEventListener('click', () => void this.run());
    this.show();
  }

  private async pick() {
    try {
      const path = await invoke<string | null>('pick_sample');
      if (!path) return;
      this.sample = path;
      localStorage.setItem('niminal-stage:gallery-sample', path);
      this.made = null;
      this.seed = 1;
      this.show();
    } catch (error) {
      this.options.status(String(error));
    }
  }

  private async run() {
    if (!this.sample || this.busy) return;
    this.busy = true;
    this.stop();
    if (this.made) this.seed++;
    this.show('making…');
    try {
      const chosen = this.checks.filter(c => c.checked).map(c => c.value);
      this.made = await invoke<Made>('gallery_make', {
        sample: this.sample,
        families: chosen.length === families.length ? [] : chosen,
        amount: this.amount.value,
        seed: this.seed,
      });
    } catch (error) {
      this.options.status(String(error));
    }
    this.busy = false;
    this.show();
  }

  private show(note = '') {
    const name = this.sample?.split(/[\\/]/).pop() ?? '';
    this.where.textContent = note || name || 'no sample chosen';
    this.make.disabled = this.busy || !this.sample;
    this.make.textContent = this.made ? 'Remix' : 'Make';
    this.grid.replaceChildren(...(this.made?.tiles ?? []).map(tile => this.tile(tile)));
  }

  private tile(tile: Tile) {
    const element = document.createElement('div');
    element.className = `gallery-tile family-${tile.family}`;
    element.setAttribute('role', 'listitem');
    const play = document.createElement('button');
    play.className = 'gallery-play';
    play.title = 'Hear it';
    play.innerHTML = `<strong></strong><small></small>`;
    play.querySelector('strong')!.textContent = tile.name.replace(/_/g, ' ');
    play.querySelector('small')!.textContent = summary(tile.values);
    play.addEventListener('click', () => void this.play(tile));
    const open = document.createElement('button');
    open.className = 'gallery-open';
    open.textContent = 'Open';
    open.title = 'Open its code in a tab';
    open.addEventListener('click', () => this.options.openPiece(`${this.made!.dir}/${tile.name}.nml`));
    element.append(play, open);
    if (this.playing?.name === tile.name) element.classList.add('playing');
    return element;
  }

  stop() {
    try {
      this.playing?.source.stop();
    } catch {
      // already ended
    }
    this.playing = null;
    this.grid.querySelectorAll('.playing').forEach(e => e.classList.remove('playing'));
  }

  private async play(tile: Tile) {
    const again = this.playing?.name === tile.name;
    this.stop();
    if (again || !this.made) return;
    try {
      this.context ??= new AudioContext();
      const data = await invoke<ArrayBuffer>('read_sample', { path: `${this.made.dir}/${tile.name}.wav` });
      const buffer = await this.context.decodeAudioData(data);
      const source = this.context.createBufferSource();
      source.buffer = buffer;
      source.connect(this.context.destination);
      source.onended = () => {
        if (this.playing?.source === source) this.stop();
      };
      source.start();
      this.playing = { source, name: tile.name };
      const index = this.made.tiles.findIndex(t => t.name === tile.name);
      this.grid.children[index]?.classList.add('playing');
    } catch (error) {
      this.options.status(String(error));
    }
  }
}
