import { invoke } from '@tauri-apps/api/core';
import { divider } from './divider';

const slashes = (path: string) => path.replace(/\\/g, '/');

/** The folder above a path, or null at a filesystem root. */
export function parentFolder(path: string): string | null {
  const dir = slashes(path).replace(/\/+$/, '');
  if (!dir || /^[a-z]:$/i.test(dir)) return null;
  const slash = dir.lastIndexOf('/');
  if (slash < 0) return null;
  const parent = dir.slice(0, slash);
  return /^[a-z]:$/i.test(parent) ? `${parent}/` : parent || '/';
}

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

const listing = (dir: string) => invoke<string[]>('list_files', { dir });
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
  /** Open a piece in a tab. */
  openPiece: (path: string) => void;
  status: (message: string) => void;
};

/** A row of the tree: a folder (its path ends in `/`) or a file, shown `depth` levels in. */
type Row = { path: string; depth: number; file?: string };

const isPiece = (path: string) => path.toLowerCase().endsWith('.nml');
const ROWS = 400;

/**
 * The rows to show for files under `root`: a tree with the folders in `open` opened, or, when searching, a
 * flat list of the paths containing every word.
 */
export function rowsFor(files: string[], root: string, open: Set<string>, words: string[]): Row[] {
  const rel = (file: string) => slashes(file).slice(root.length).replace(/^\//, '');
  if (words.length) {
    return files
      .filter(file => words.every(word => rel(file).toLowerCase().includes(word)))
      .map(file => ({ path: rel(file), depth: 0, file }));
  }
  const rows: Row[] = [];
  const seen = new Set<string>();
  for (const file of files) {
    const parts = rel(file).split('/');
    let hidden = false;
    for (let i = 0; i < parts.length - 1; i++) {
      const dir = `${parts.slice(0, i + 1).join('/')}/`;
      if (!hidden && !seen.has(dir)) rows.push({ path: dir, depth: i });
      seen.add(dir);
      if (!open.has(dir)) hidden = true;
    }
    if (!hidden) rows.push({ path: parts.join('/'), depth: parts.length - 1, file });
  }
  return rows;
}

/**
 * A panel beside the code to find the pieces and samples of a folder: pieces open in tabs, samples are heard,
 * seen, and written into the code at the cursor, which stays where it was put in the editor.
 */
export class FileBrowse {
  readonly element: HTMLElement;
  private folder: string | null = localStorage.getItem('niminal-stage:samples');
  private files: string[] = [];
  private rows: Row[] = [];
  private opened = new Set<string>();
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
    this.element.id = 'files';
    this.element.hidden = true;
    this.element.innerHTML = `
      <div class="files-divider" role="separator" aria-orientation="vertical" aria-label="Resize file browser" tabindex="0" title="Drag to resize · double-click to reset"></div>
      <div class="files-head">
        <strong>browse</strong>
        <span class="files-where"></span>
        <button class="files-up" title="Parent folder (← at top level)" disabled>↑ Up</button>
        <button class="files-folder" title="Choose another folder">Folder…</button>
        <button class="files-close" title="Close (Esc)">×</button>
      </div>
      <div class="files-actions">
        <button id="new" title="A new empty tab (Cmd/Ctrl+T)">New</button>
        <button id="open" title="Open a .nml file in a tab">Open…</button>
        <button id="open-project" title="Open a folder: main.nml and what it uses, or every .nml file, as tabs">Project…</button>
        <button id="save" title="Save the file">Save</button>
      </div>
      <input class="files-filter" placeholder="filter · ↑↓ · ↩ opens or inserts" spellcheck="false" />
      <div class="files-list" role="tree" tabindex="0" aria-label="Files"></div>
      <canvas class="files-wave" width="440" height="56"></canvas>
      <div class="files-detail"></div>`;
    const find = <T extends HTMLElement>(selector: string) => this.element.querySelector<T>(selector)!;
    this.list = find('.files-list');
    this.filter = find<HTMLInputElement>('.files-filter');
    this.where = find('.files-where');
    this.wave = find<HTMLCanvasElement>('.files-wave');
    this.detail = find('.files-detail');
    find('.files-close').addEventListener('click', () => this.close());
    find('.files-up').addEventListener('click', () => void this.up());
    find('.files-folder').addEventListener('click', () => void this.choose());
    const widthKey = 'niminal-stage:browser-width';
    const savedWidth = Number(localStorage.getItem(widthKey));
    if (savedWidth > 0) this.element.style.setProperty('--browser-width', `${savedWidth}px`);
    const resize = (width: number) => {
      const available = this.element.parentElement?.clientWidth ?? 0;
      const clamped = Math.round(Math.min(Math.max(width, 220), Math.max(220, available - 240)));
      this.element.style.setProperty('--browser-width', `${clamped}px`);
      localStorage.setItem(widthKey, String(clamped));
    };
    divider(find('.files-divider'), 1, () => this.element.getBoundingClientRect().width, resize, () => {
      this.element.style.removeProperty('--browser-width');
      localStorage.removeItem(widthKey);
    });
    this.filter.addEventListener('input', () => this.refilter());
    const navigate = (event: KeyboardEvent) => {
      const row = this.rows[this.selected];
      const folder = row && !row.file;
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        this.select(this.selected + (event.key === 'ArrowDown' ? 1 : -1));
      } else if (event.key === 'ArrowRight' && folder) {
        event.preventDefault();
        if (!this.opened.has(row.path)) this.toggleDir(row.path);
        else if (this.rows[this.selected + 1]?.depth > row.depth) this.select(this.selected + 1);
      } else if (event.key === 'ArrowLeft' && !this.words().length) {
        event.preventDefault();
        if (folder && this.opened.has(row.path)) this.toggleDir(row.path);
        else if (!row || row.depth === 0) void this.up();
        else {
          for (let i = this.selected - 1; i >= 0; i--) {
            if (this.rows[i].depth < row.depth) { this.select(i); break; }
          }
        }
      } else if (event.key === 'Enter') {
        event.preventDefault();
        this.activate();
      } else if (event.key === 'Escape') {
        event.preventDefault();
        this.close();
      }
    };
    this.filter.addEventListener('keydown', navigate);
    this.list.addEventListener('keydown', navigate);
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

  private root() {
    return slashes(this.folder ?? this.options.fileDir() ?? '').replace(/\/$/, '');
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
    this.opened.clear();
    await this.load(dir);
    this.filter.focus();
  }

  private async up() {
    const root = this.root();
    const parent = parentFolder(root);
    if (!parent) return;
    this.stop();
    this.folder = parent;
    localStorage.setItem('niminal-stage:samples', parent);
    this.opened.clear();
    this.filter.value = '';
    await this.load(parent);
    const previous = root.slice(root.lastIndexOf('/') + 1) + '/';
    const index = this.rows.findIndex(row => row.path === previous);
    if (index >= 0) this.select(index);
    this.list.focus();
  }

  private async load(dir: string) {
    this.where.textContent = dir;
    this.where.title = dir;
    this.element.querySelector<HTMLButtonElement>('.files-up')!.disabled = parentFolder(dir) === null;
    try {
      this.files = await listing(dir);
    } catch (error) {
      this.options.status(String(error));
      this.files = [];
    }
    this.refilter();
  }

  private words() {
    return this.filter.value.toLowerCase().split(/\s+/).filter(Boolean);
  }

  private refilter() {
    this.rows = rowsFor(this.files, this.root(), this.opened, this.words()).slice(0, ROWS);
    this.render();
    this.selected = -1;
    this.clearWave();
    const count = this.rows.filter(row => row.file).length;
    this.detail.textContent = this.rows.length
      ? `${count}${this.rows.length === ROWS ? '+' : ''} file${count === 1 ? '' : 's'} shown`
      : this.files.length
        ? 'nothing matches'
        : 'no pieces or audio in this folder';
  }

  private render() {
    const searching = this.words().length > 0;
    this.list.replaceChildren(
      ...this.rows.map((row, index) => {
        const el = document.createElement('div');
        el.className = !row.file ? 'entry dir' : isPiece(row.path) ? 'entry piece' : 'entry';
        el.setAttribute('role', 'treeitem');
        el.style.paddingLeft = `calc(${6 + row.depth * 12}px * var(--z))`;
        const name = searching ? row.path : (row.path.replace(/\/$/, '').split('/').pop() ?? row.path);
        el.textContent = row.file ? name : `${this.opened.has(row.path) ? '▾' : '▸'} ${name}`;
        el.title = row.file ?? row.path;
        el.addEventListener('click', () => {
          this.list.focus();
          this.select(index);
          if (!row.file) this.toggleDir(row.path);
        });
        el.addEventListener('dblclick', () => this.activate());
        return el;
      }),
    );
  }

  private toggleDir(dir: string) {
    if (!this.opened.delete(dir)) this.opened.add(dir);
    this.rows = rowsFor(this.files, this.root(), this.opened, []).slice(0, ROWS);
    this.render();
    this.mark(Math.max(0, this.rows.findIndex(row => row.path === dir)));
  }

  private mark(index: number) {
    this.selected = index;
    this.list.querySelectorAll('.entry').forEach((el, i) => el.classList.toggle('on', i === index));
    this.list.children[index]?.scrollIntoView({ block: 'nearest' });
  }

  private select(index: number) {
    if (!this.rows.length) return;
    this.mark(Math.min(Math.max(index, 0), this.rows.length - 1));
    const row = this.rows[this.selected];
    if (row.file && !isPiece(row.path)) {
      void this.audition(row.file);
      return;
    }
    this.stop();
    this.clearWave();
    this.detail.textContent = row.file ? 'a piece: ↩ opens it in a tab' : '';
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

  private async audition(file: string) {
    this.stop();
    const heard = this.heard;
    try {
      this.context ??= new AudioContext();
      const data = await bytesOf(file);
      const buffer = await this.context.decodeAudioData(data);
      // another sample was chosen while this one loaded
      if (heard !== this.heard) return;
      const source = this.context.createBufferSource();
      source.buffer = buffer;
      source.connect(this.context.destination);
      source.start();
      this.playing = source;
      this.draw(buffer.getChannelData(0));
      this.detail.textContent = `${buffer.duration.toFixed(2)}s · ${buffer.numberOfChannels === 1 ? 'mono' : `${buffer.numberOfChannels} channels`} · ${Math.round(buffer.sampleRate)} Hz · ↩ inserts it`;
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

  /** Toggle a folder, open a piece, or write a sample into the code. */
  private activate() {
    const row = this.rows[this.selected];
    if (!row) return;
    if (!row.file) {
      this.toggleDir(row.path);
      return;
    }
    if (isPiece(row.path)) {
      this.options.openPiece(row.file);
      return;
    }
    const path = relativePath(this.options.fileDir(), row.file);
    this.options.insert((before, after) => snippetFor(before, after, path));
    this.options.status(`inserted sample("${path}")`);
  }
}
