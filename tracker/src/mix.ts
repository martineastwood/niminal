import type { Output, Project, Track } from './project';
import { stripOf, returnStrip, returnIndex } from './effects';
import { maxReturns } from './project';
import { trackColour } from './grid';
import { drawScope, meterLevel } from './scope';

export interface MixActions {
  /** Select a track in the pattern editor. */
  selectTrack(track: number): void;
  /** Open a strip's effect chain; the master is -1 and returns are below that. */
  openFx(track: number): void;
  /** A level, pan, output, mute or solo changed. */
  edited(): void;
  addReturn(): void;
  removeReturn(index: number): void;
}

const dbText = (db: number) => `${db > -59.95 ? (Math.round(db * 10) / 10).toFixed(1) : '-∞'}`;

/** A vertical fader, from `min` at the bottom to `max` at the top. */
function fader(label: string, value: number, min: number, max: number, change: (value: number) => void): HTMLElement {
  const element = document.createElement('div'); element.className = 'fader'; element.tabIndex = 0;
  element.setAttribute('role', 'slider'); element.setAttribute('aria-label', label);
  element.setAttribute('aria-valuemin', String(min)); element.setAttribute('aria-valuemax', String(max));
  const fill = document.createElement('div'); fill.className = 'fader-fill'; const handle = document.createElement('div'); handle.className = 'fader-handle'; element.append(fill, handle);
  const show = (v: number) => { const percent = (v - min) / (max - min) * 100; fill.style.height = `${percent}%`; handle.style.bottom = `calc(${percent}% - 3px)`; element.setAttribute('aria-valuenow', String(Math.round(v * 10) / 10)); };
  const set = (v: number) => { value = Math.min(max, Math.max(min, v)); show(value); change(value); };
  const fromPointer = (event: PointerEvent) => { const box = element.getBoundingClientRect(); set(min + (1 - (event.clientY - box.top) / box.height) * (max - min)); };
  element.onpointerdown = event => { element.setPointerCapture(event.pointerId); fromPointer(event); element.onpointermove = fromPointer; };
  element.onpointerup = element.onpointercancel = () => { element.onpointermove = null; };
  element.ondblclick = () => set(0);
  element.onkeydown = event => {
    const step = event.shiftKey ? 0.1 : 0.5;
    if (event.key === 'ArrowUp' || event.key === 'ArrowRight') set(value + step); else if (event.key === 'ArrowDown' || event.key === 'ArrowLeft') set(value - step); else return;
    event.preventDefault();
  };
  show(value);
  return element;
}

/** The mixer: a strip for each track, return and the master, each with its scope, level, pan, mute and solo. */
export class MixPanel {
  private strips: { track: number; element: HTMLElement; canvas: HTMLCanvasElement; meter: HTMLElement }[] = [];
  private active = 0;

  constructor(private parent: HTMLElement, private project: () => Project, private actions: MixActions) {}

  highlight(track: number) {
    this.active = track;
    for (const strip of this.strips) strip.element.classList.toggle('selected', strip.track === track);
  }

  render() {
    const project = this.project();
    this.parent.replaceChildren(); this.strips = [];
    const add = (index: number) => {
      const track = stripOf(project, index), master = index === -1, ret = index < -1;
      const element = document.createElement('div'); element.className = `mix-strip ${master ? 'master' : ''} ${ret ? 'return' : ''}`; element.dataset.track = String(index);
      element.style.setProperty('--track-colour', master ? '#d9903f' : ret ? '#5fa8a0' : trackColour(track, index)); element.classList.toggle('selected', index === this.active);
      const name = document.createElement('button'); name.className = 'mix-name'; name.textContent = master ? 'Master' : ret ? track.name : `${String(index + 1).padStart(2, '0')} ${track.name}`; name.title = track.name;
      name.onclick = () => (index < 0 ? this.actions.openFx(index) : this.actions.selectTrack(index));
      const canvas = document.createElement('canvas'); canvas.className = 'mix-scope'; canvas.setAttribute('aria-label', `${track.name} scope`);
      const pan = document.createElement('input'); pan.type = 'range'; pan.min = '-1'; pan.max = '1'; pan.step = '0.01'; pan.value = String(track.pan); pan.className = 'mix-pan'; pan.setAttribute('aria-label', `${track.name} pan`);
      pan.oninput = () => { track.pan = Number(pan.value); this.actions.edited(); }; pan.ondblclick = () => { pan.value = '0'; pan.oninput!(new Event('input')); };
      const readout = document.createElement('span'); readout.className = 'mix-db'; readout.textContent = dbText(track.volume_db);
      const level = fader(`${track.name} volume in dB`, track.volume_db, -60, 6, value => { track.volume_db = Math.round(value * 10) / 10; readout.textContent = dbText(track.volume_db); this.actions.edited(); });
      const meter = document.createElement('div'); meter.className = 'meter'; const fill = document.createElement('div'); fill.className = 'meter-fill'; meter.append(fill);
      const faders = document.createElement('div'); faders.className = 'mix-faders'; faders.append(level, meter);
      const buttons = document.createElement('div'); buttons.className = 'mix-buttons';
      if (!master) {
        const toggle = (text: string, label: string, key: 'muted' | 'solo') => {
          const b = document.createElement('button'); b.textContent = text; b.title = label; b.setAttribute('aria-label', `${label} ${track.name}`); b.setAttribute('aria-pressed', String(!!track[key]));
          b.onclick = () => { track[key] = !track[key]; this.actions.edited(); this.render(); }; return b;
        };
        buttons.append(toggle('M', 'Mute', 'muted')); if (!ret) buttons.append(toggle('S', 'Solo', 'solo'));
      }
      const fx = document.createElement('button'); fx.textContent = track.fx.length ? `FX ${track.fx.length}` : 'FX'; fx.title = `${track.name} effects`; fx.setAttribute('aria-label', `${track.name} effect chain`);
      fx.onclick = () => this.actions.openFx(index); buttons.append(fx);
      if (ret) { const remove = document.createElement('button'); remove.textContent = '×'; remove.title = `Remove ${track.name}`; remove.setAttribute('aria-label', `Remove ${track.name}`); remove.onclick = () => this.actions.removeReturn(returnIndex(index)); buttons.append(remove); }
      element.append(name, canvas, pan, faders, readout, buttons);
      if (!master) element.append(this.output(track, project, ret ? returnIndex(index) + 1 : 0));
      this.parent.append(element); this.strips.push({ track: index, element, canvas, meter: fill });
    };
    for (let i = 0; i < project.tracks.length; i++) add(i);
    const gap = document.createElement('div'); gap.className = 'mix-gap'; this.parent.append(gap);
    for (let r = 0; r < project.returns.length; r++) add(returnStrip(r));
    if (project.returns.length < maxReturns) {
      const create = document.createElement('button'); create.className = 'mix-add-return'; create.textContent = '+ Return'; create.title = 'Add a return track for sends'; create.onclick = () => this.actions.addReturn(); this.parent.append(create);
    }
    add(-1);
  }

  /** Where the strip's signal goes: the master, a return from `first` on (a return only feeds later ones), or nowhere. */
  private output(track: Track, project: Project, first: number) {
    const select = document.createElement('select'); select.className = 'mix-output'; select.setAttribute('aria-label', `${track.name} output`); select.title = `${track.name} output`;
    const choices: [string, Output][] = [['Master', { kind: 'master' }], ...project.returns.flatMap((r, i): [string, Output][] => i >= first ? [[r.name, { kind: 'return', index: i }]] : []), ['None', { kind: 'none' }]];
    if (track.output?.kind === 'return' && track.output.index >= project.returns.length) choices.push(['No return', track.output]);
    const key = (o: Output) => o.kind === 'return' ? `return${o.index}` : o.kind;
    choices.forEach(([text, output]) => select.add(new Option(text, key(output))));
    select.value = key(track.output ?? { kind: 'master' });
    select.onchange = () => { track.output = choices.find(([, o]) => key(o) === select.value)![1]; this.actions.edited(); };
    return select;
  }

  /** Draws the latest scopes (`s_<track>` and `s_m`). */
  paint(scopes: Record<string, number[]>) {
    for (const strip of this.strips) {
      const frames = scopes[`s_${strip.track >= 0 ? strip.track : strip.track === -1 ? 'm' : `r${returnIndex(strip.track)}`}`];
      drawScope(strip.canvas, frames, strip.track === -1 ? '#d9903f' : getComputedStyle(strip.element).getPropertyValue('--track-colour') || '#9fb0bf');
      strip.meter.style.height = `${meterLevel(frames) * 100}%`;
    }
  }
}
