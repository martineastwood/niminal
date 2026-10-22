import type { Project } from './project';
import { stripOf } from './effects';
import { trackColour } from './grid';
import { drawScope, meterLevel } from './scope';

export interface MixActions {
  /** Select a track in the pattern editor. */
  selectTrack(track: number): void;
  /** Open a strip's effect chain; the master is -1. */
  openFx(track: number): void;
  /** A level, pan, mute or solo changed. */
  edited(): void;
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

/** The mixer: a strip for each track and for the master, each with its scope, level, pan, mute and solo. */
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
      const track = stripOf(project, index), master = index < 0;
      const element = document.createElement('div'); element.className = `mix-strip ${master ? 'master' : ''}`; element.dataset.track = String(index);
      element.style.setProperty('--track-colour', master ? '#d9903f' : trackColour(track, index)); element.classList.toggle('selected', index === this.active);
      const name = document.createElement('button'); name.className = 'mix-name'; name.textContent = master ? 'Master' : `${String(index + 1).padStart(2, '0')} ${track.name}`; name.title = track.name;
      name.onclick = () => (master ? this.actions.openFx(-1) : this.actions.selectTrack(index));
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
        buttons.append(toggle('M', 'Mute', 'muted'), toggle('S', 'Solo', 'solo'));
      }
      const fx = document.createElement('button'); fx.textContent = track.fx.length ? `FX ${track.fx.length}` : 'FX'; fx.title = `${track.name} effects`; fx.setAttribute('aria-label', `${track.name} effect chain`);
      fx.onclick = () => this.actions.openFx(index); buttons.append(fx);
      element.append(name, canvas, pan, faders, readout, buttons);
      this.parent.append(element); this.strips.push({ track: index, element, canvas, meter: fill });
    };
    for (let i = 0; i < project.tracks.length; i++) add(i);
    const gap = document.createElement('div'); gap.className = 'mix-gap'; this.parent.append(gap);
    add(-1);
  }

  /** Draws the latest scopes (`s_<track>` and `s_m`). */
  paint(scopes: Record<string, number[]>) {
    for (const strip of this.strips) {
      const frames = scopes[`s_${strip.track < 0 ? 'm' : strip.track}`];
      drawScope(strip.canvas, frames, strip.track < 0 ? '#d9903f' : getComputedStyle(strip.element).getPropertyValue('--track-colour') || '#9fb0bf');
      strip.meter.style.height = `${meterLevel(frames) * 100}%`;
    }
  }
}
