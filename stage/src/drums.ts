import { invoke } from '@tauri-apps/api/core';

/** What the generator is asked for. A drum's scale makes it busier (above 1) or sparser (below 1). */
export type DrumRequest = {
  theme: string;
  density: number;
  chaos: number;
  seed: number;
  scales: Record<string, number>;
};
export type DrumRow = { name: string; steps: string };

const num = (n: number) => String(Number(n.toFixed(2)));

/** The call that makes the grid in a clip: `drum_grid(theme: house, density: 0.8, seed: 4, hat: 0.6)`. */
export function drumCall(r: DrumRequest) {
  const drums = Object.entries(r.scales)
    .filter(([, scale]) => scale !== 1)
    .map(([name, scale]) => `, ${name}: ${num(scale)}`);
  const chaos = r.chaos > 0 ? `, chaos: ${num(r.chaos)}` : '';
  return `drum_grid(theme: ${r.theme}, density: ${num(r.density)}${chaos}, seed: ${r.seed}${drums.join('')})`;
}

/** The rows written out as a `grid { }`, in groups of four steps, to edit by hand. */
export function gridCode(rows: DrumRow[]) {
  const width = Math.max(0, ...rows.map(r => r.name.length)) + 1;
  const lines = rows.map(r => {
    const groups = r.steps.match(/.{1,4}/g) ?? [];
    return `  ${`${r.name}:`.padEnd(width + 1)}${groups.join(' ')}`;
  });
  return `grid {\n${lines.join('\n')}\n}`;
}

/** What the Lab plays to let you hear the grid: the shelf's drums in a kit of the generator's row names. */
export function auditionDrums(r: DrumRequest) {
  return [
    'use std/drums.{kick_deep, snare_tight, clap, hat_closed, hat_open, rim}',
    '__lab_kit = kit { kick: kick_deep, snare: snare_tight, clap: clap, hat: hat_closed, open: hat_open, perc: rim }',
    `clip __lab_drums {\n  instrument: __lab_kit\n  notes: ${drumCall(r)}\n}`,
    '__lab_drums.start',
  ].join('\n');
}

export const stopDrums = '__lab_drums.stop';

type Options = {
  evaluate: (source: string) => Promise<void>;
  insert: (text: string) => void;
  status: (message: string) => void;
};

/** The drum generator: pick a theme, shape it, hear it, and put the code in the piece. */
export class Drums {
  readonly element: HTMLElement;
  private request: DrumRequest = { theme: 'techno', density: 0.7, chaos: 0, seed: 1, scales: {} };
  private rows: DrumRow[] = [];
  private hearing = false;
  private theme: HTMLSelectElement;
  private grid: HTMLElement;
  private hear: HTMLButtonElement;
  private seed: HTMLElement;
  private readouts = new Map<string, HTMLOutputElement>();
  private timer: number | undefined;

  constructor(private options: Options) {
    this.element = document.createElement('div');
    this.element.className = 'drums';
    this.element.innerHTML = `
      <div class="drums-head">
        <select class="drums-theme" title="The groove to start from"></select>
        <button class="drums-hear primary" title="Hear it. Changes you make while it plays are heard as you make them">Hear</button>
      </div>
      <div class="drums-sliders"></div>
      <div class="drums-head">
        <span class="drums-seed" title="The same settings and seed always make the same grid"></span>
        <button class="drums-another" title="Another grid with the same settings">Another</button>
      </div>
      <div class="drums-grid" role="table"></div>
      <div class="drums-head">
        <button class="drums-call" title="Put drum_grid(...) in the editor, for a clip's notes">Insert call</button>
        <button class="drums-code" title="Put the grid in the editor as text to edit by hand">Insert grid</button>
      </div>`;
    const find = <T extends HTMLElement>(selector: string) => this.element.querySelector<T>(selector)!;
    this.theme = find('.drums-theme');
    this.grid = find('.drums-grid');
    this.hear = find('.drums-hear');
    this.seed = find('.drums-seed');
    const sliders = find('.drums-sliders');
    for (const [key, label] of [['density', 'density'], ['chaos', 'chaos']] as const) {
      sliders.append(this.slider(label, 0, 1, this.request[key], value => {
        this.request[key] = value;
        this.changed();
      }));
    }
    this.theme.addEventListener('change', () => {
      this.request.theme = this.theme.value;
      this.request.scales = {};
      this.changed();
    });
    find('.drums-another').addEventListener('click', () => {
      this.request.seed++;
      this.changed();
    });
    this.hear.addEventListener('click', () => void this.toggle());
    find('.drums-call').addEventListener('click', () => this.options.insert(drumCall(this.request)));
    find('.drums-code').addEventListener('click', () => this.options.insert(gridCode(this.rows)));
  }

  private slider(label: string, min: number, max: number, value: number, set: (v: number) => void) {
    const row = document.createElement('label');
    row.className = 'drums-slider';
    const name = document.createElement('span');
    name.textContent = label;
    const input = document.createElement('input');
    input.type = 'range';
    input.min = String(min * 100);
    input.max = String(max * 100);
    input.value = String(Math.round(value * 100));
    const readout = document.createElement('output');
    readout.textContent = num(value);
    this.readouts.set(label, readout);
    input.addEventListener('input', () => {
      const v = Number(input.value) / 100;
      readout.textContent = num(v);
      set(v);
    });
    input.addEventListener('dblclick', () => {
      input.value = String(Math.round(value * 100));
      readout.textContent = num(value);
      set(value);
    });
    row.append(name, input, readout);
    return row;
  }

  async open() {
    if (this.theme.options.length === 0) {
      try {
        const themes = await invoke<string[]>('drums_themes');
        this.theme.replaceChildren(
          ...themes.map(t => Object.assign(document.createElement('option'), { value: t, textContent: t.replace(/_/g, ' ') })),
        );
        this.theme.value = this.request.theme;
      } catch (error) {
        this.options.status(String(error));
        return;
      }
    }
    await this.refresh();
  }

  stop() {
    if (!this.hearing) return;
    this.hearing = false;
    this.hear.textContent = 'Hear';
    this.hear.classList.remove('on');
    void this.options.evaluate(stopDrums);
  }

  private async toggle() {
    if (this.hearing) return this.stop();
    this.hearing = true;
    this.hear.textContent = 'Stop';
    this.hear.classList.add('on');
    await this.options.evaluate(auditionDrums(this.request));
  }

  /** A setting moved: show the new grid, and if it is sounding, send it so the change is heard. */
  private changed() {
    window.clearTimeout(this.timer);
    this.timer = window.setTimeout(() => void this.refresh(true), 60);
  }

  private async refresh(send = false) {
    this.seed.textContent = `seed ${this.request.seed}`;
    try {
      this.rows = await invoke<DrumRow[]>('drums_make', {
        theme: this.request.theme,
        density: this.request.density,
        chaos: this.request.chaos,
        seed: this.request.seed,
        perDrum: Object.entries(this.request.scales),
      });
    } catch (error) {
      this.options.status(String(error));
      return;
    }
    this.grid.replaceChildren(...this.rows.map(row => this.row(row)));
    if (send && this.hearing) await this.options.evaluate(auditionDrums(this.request));
  }

  private row(row: DrumRow) {
    const line = document.createElement('div');
    line.className = 'drums-row';
    const name = document.createElement('span');
    name.className = 'drums-name';
    name.textContent = row.name;
    const steps = document.createElement('span');
    steps.className = 'drums-steps';
    for (const [i, c] of [...row.steps].entries()) {
      const cell = document.createElement('i');
      cell.className = c === 'x' ? 'hit' : c === 'o' ? 'ghost' : '';
      if (i % 4 === 0) cell.classList.add('beat');
      steps.append(cell);
    }
    const scale = document.createElement('input');
    scale.type = 'range';
    scale.className = 'drums-scale';
    scale.title = `How busy ${row.name} is compared with the rest`;
    scale.min = '0';
    scale.max = '150';
    scale.value = String(Math.round((this.request.scales[row.name] ?? 1) * 100));
    scale.addEventListener('input', () => {
      this.request.scales[row.name] = Number(scale.value) / 100;
      this.changed();
    });
    line.append(name, steps, scale);
    return line;
  }
}
