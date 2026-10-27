import { invoke } from '@tauri-apps/api/core';

export type SampleFile = { path: string; bytes: number };

const slashes = (path: string) => path.replace(/\\/g, '/');

/** `file` as a path from folder `from`, the way a piece names its samples; the full path when there is no folder. */
export function relativePath(from: string | null, file: string) {
  const target = slashes(file);
  if (!from) return target;
  const base = slashes(from).replace(/\/$/, '').split('/');
  const parts = target.split('/');
  let shared = 0;
  while (shared < base.length && shared < parts.length - 1 && base[shared] === parts[shared]) shared++;
  if (shared === 0) return target;
  return [...Array<string>(base.length - shared).fill('..'), ...parts.slice(shared)].join('/');
}

/** A name a piece can bind a sample to: `Kick 01.wav` becomes `kick_01`. */
export function nameFor(file: string) {
  const stem = (slashes(file).split('/').pop() ?? file).replace(/\.[^.]*$/, '');
  const name = stem.toLowerCase().replace(/[^a-z0-9_]+/g, '_').replace(/^_+|_+$/g, '');
  return /^[a-z]/.test(name) ? name : `s_${name || 'sample'}`;
}

/**
 * What to write for a sample: just the call where something is already being assigned or passed
 * (`kick = ▮`), and a whole binding on an empty line.
 */
export function snippetFor(lineBefore: string, lineAfter: string, path: string) {
  const call = `sample("${path}")`;
  const blank = lineBefore.trim() === '' && lineAfter.trim() === '';
  return blank ? `${nameFor(path)} = ${call}` : call;
}

const listing = (dir: string) => invoke<SampleFile[]>('list_samples', { dir });
const pickFolder = () => invoke<string | null>('pick_folder');
const bytesOf = (path: string) => invoke<ArrayBuffer>('read_sample', { path });

/** One column of a waveform per pixel: the quietest and loudest sample in it. */
export function peaksOf(channel: Float32Array, columns: number) {
  const out: [number, number][] = [];
  const size = Math.max(1, Math.floor(channel.length / columns));
  for (let c = 0; c < columns; c++) {
    let low = 0;
    let high = 0;
    for (let i = c * size; i < Math.min((c + 1) * size, channel.length); i++) {
      low = Math.min(low, channel[i]);
      high = Math.max(high, channel[i]);
    }
    out.push([low, high]);
  }
  return out;
}

type Options = {
  /** The folder of the file being edited, if it has been saved: paths are written from it. */
  fileDir: () => string | null;
  /** The text before and after the cursor on its line, and a way to put text there. */
  insert: (make: (before: string, after: string) => string) => void;
  status: (message: string) => void;
};

/** A panel to find samples in a folder, hear them, see their shape, and write them into the code. */
export class SampleBrowser {
  readonly element: HTMLElement;
  private folder: string | null = localStorage.getItem('niminal-stage:samples');
  private files: SampleFile[] = [];
  private shown: SampleFile[] = [];
  private selected = -1;
  private context: AudioContext | null = null;
  private playing: AudioBufferSourceNode | null = null;
  private heard = 0;
  private list: HTMLElement;
  private filter: HTMLInputElement;
  private where: HTMLElement;
  private wave: HTMLCanvasElement;
  private detail: HTMLElement;

  constructor(private options: Options) {
    this.element = document.createElement('aside');
    this.element.id = 'samples';
    this.element.hidden = true;
    this.element.innerHTML = `
      <div class="samples-head">
        <strong>samples</strong>
        <span class="samples-where"></span>
        <button class="samples-folder" title="Choose another folder">Folder…</button>
        <button class="samples-close" title="Close (Esc)">×</button>
      </div>
      <input class="samples-filter" placeholder="filter · ↑↓ to hear · ↩ to insert" spellcheck="false" />
      <div class="samples-list" role="listbox"></div>
      <canvas class="samples-wave" width="440" height="56"></canvas>
      <div class="samples-detail"></div>`;
    const find = <T extends HTMLElement>(selector: string) => this.element.querySelector<T>(selector)!;
    this.list = find('.samples-list');
    this.filter = find<HTMLInputElement>('.samples-filter');
    this.where = find('.samples-where');
    this.wave = find<HTMLCanvasElement>('.samples-wave');
    this.detail = find('.samples-detail');
    find('.samples-close').addEventListener('click', () => this.close());
    find('.samples-folder').addEventListener('click', () => void this.choose());
    this.filter.addEventListener('input', () => this.refilter());
    this.filter.addEventListener('keydown', event => {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        this.select(this.selected + (event.key === 'ArrowDown' ? 1 : -1));
      } else if (event.key === 'Enter') {
        event.preventDefault();
        this.insertSelected();
      } else if (event.key === 'Escape') {
        event.preventDefault();
        this.close();
      }
    });
  }

  get open() {
    return !this.element.hidden;
  }

  toggle() {
    if (this.open) this.close();
    else void this.show();
  }

  close() {
    this.stop();
    this.element.hidden = true;
  }

  private async show() {
    this.element.hidden = false;
    this.filter.focus();
    // the folder of the file being edited unless one was chosen
    const dir = this.folder ?? this.options.fileDir();
    if (dir) await this.load(dir);
    else this.where.textContent = 'choose a folder';
  }

  private async choose() {
    const dir = await pickFolder();
    if (!dir) return;
    this.folder = dir;
    localStorage.setItem('niminal-stage:samples', dir);
    await this.load(dir);
    this.filter.focus();
  }

  private async load(dir: string) {
    this.where.textContent = dir;
    this.where.title = dir;
    try {
      this.files = await listing(dir);
    } catch (error) {
      this.options.status(String(error));
      this.files = [];
    }
    this.refilter();
  }

  private refilter() {
    const words = this.filter.value.toLowerCase().split(/\s+/).filter(Boolean);
    const root = slashes(this.folder ?? this.options.fileDir() ?? '');
    this.shown = this.files
      .filter(file => words.every(word => slashes(file.path).toLowerCase().includes(word)))
      .slice(0, 300);
    this.list.replaceChildren(
      ...this.shown.map((file, index) => {
        const row = document.createElement('div');
        row.className = 'sample';
        row.setAttribute('role', 'option');
        row.textContent = slashes(file.path).replace(root, '').replace(/^\//, '');
        row.title = file.path;
        row.addEventListener('click', () => this.select(index));
        row.addEventListener('dblclick', () => this.insertSelected());
        return row;
      }),
    );
    this.selected = -1;
    this.detail.textContent = this.shown.length
      ? `${this.shown.length}${this.shown.length === 300 ? '+' : ''} sample${this.shown.length === 1 ? '' : 's'}`
      : this.files.length
        ? 'nothing matches'
        : 'no audio files in this folder';
    this.clearWave();
  }

  private select(index: number) {
    if (!this.shown.length) return;
    this.selected = Math.min(Math.max(index, 0), this.shown.length - 1);
    this.list.querySelectorAll('.sample').forEach((row, i) => row.classList.toggle('on', i === this.selected));
    this.list.children[this.selected]?.scrollIntoView({ block: 'nearest' });
    void this.audition(this.shown[this.selected]);
  }

  private stop() {
    this.heard++;
    try {
      this.playing?.stop();
    } catch {
      // already finished
    }
    this.playing = null;
  }

  private async audition(file: SampleFile) {
    this.stop();
    const heard = this.heard;
    try {
      this.context ??= new AudioContext();
      const data = await bytesOf(file.path);
      const buffer = await this.context.decodeAudioData(data);
      // another sample was chosen while this one loaded
      if (heard !== this.heard) return;
      const source = this.context.createBufferSource();
      source.buffer = buffer;
      source.connect(this.context.destination);
      source.start();
      this.playing = source;
      this.draw(buffer.getChannelData(0));
      this.detail.textContent = `${buffer.duration.toFixed(2)}s · ${buffer.numberOfChannels === 1 ? 'mono' : `${buffer.numberOfChannels} channels`} · ${Math.round(buffer.sampleRate)} Hz`;
    } catch (error) {
      if (heard === this.heard) {
        this.clearWave();
        this.detail.textContent = `can't play that: ${error}`;
      }
    }
  }

  private clearWave() {
    const g = this.wave.getContext('2d');
    g?.clearRect(0, 0, this.wave.width, this.wave.height);
  }

  private draw(channel: Float32Array) {
    const g = this.wave.getContext('2d');
    if (!g) return;
    this.clearWave();
    g.fillStyle = getComputedStyle(this.element).getPropertyValue('--accent') || '#e9b66e';
    const mid = this.wave.height / 2;
    peaksOf(channel, this.wave.width).forEach(([low, high], x) => {
      g.fillRect(x, mid - high * mid, 1, Math.max(1, (high - low) * mid));
    });
  }

  private insertSelected() {
    const file = this.shown[this.selected];
    if (!file) return;
    const path = relativePath(this.options.fileDir(), file.path);
    this.options.insert((before, after) => snippetFor(before, after, path));
    this.options.status(`inserted sample("${path}")`);
  }
}
