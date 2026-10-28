import type { Clip } from './live';

/** One control as the engine reports it: `ctl` values and the parameters of effects. */
export type ControlInfo = {
  name: string;
  value: number;
  target: number;
  unit: string | null;
  /** What it moves over, in the units of `value`; null when the code gives no range. */
  range: [number, number] | null;
  /** Moves on a log scale, as a frequency does. */
  log: boolean;
  /** What drives it, such as `midi.cc(21)`; null when nothing does. */
  source: string | null;
  kind: 'ctl' | 'fx';
};

/** A clip that only moves the controls of an effect plays through this instrument. */
export const gestureInstrument = '__gesture';

export const isGesture = (clip: Clip) => clip.instrument === gestureInstrument;

/** A control bound to a key is open while the key is down, so it is played with a held pad. */
export const isGate = (control: ControlInfo) => control.source?.startsWith('midi.note') ?? false;

/** The span a slider covers: the control's range, or 0 to 1, widened to hold the value it has. */
export function span(control: ControlInfo): [number, number] {
  let [lo, hi] = control.range ?? [0, 1];
  if (control.range === null) {
    lo = Math.min(lo, control.value);
    hi = Math.max(hi, control.value);
  }
  return hi > lo ? [lo, hi] : [lo, lo + 1];
}

const logged = (control: ControlInfo, lo: number) => control.log && lo > 0;

/** Where a value sits on its slider, 0 to 1. */
export function toPosition(control: ControlInfo, value: number) {
  const [lo, hi] = span(control);
  const t = logged(control, lo) ? Math.log(Math.max(value, lo) / lo) / Math.log(hi / lo) : (value - lo) / (hi - lo);
  return Math.min(1, Math.max(0, t));
}

/** The value at a place on the slider, 0 to 1. */
export function fromPosition(control: ControlInfo, t: number) {
  const [lo, hi] = span(control);
  const at = Math.min(1, Math.max(0, t));
  return logged(control, lo) ? lo * (hi / lo) ** at : lo + (hi - lo) * at;
}

/** A value for reading: a few digits, and its unit. */
export function format(control: ControlInfo, value: number) {
  const size = Math.abs(value);
  const digits = size >= 1000 ? 0 : size >= 100 ? 1 : size >= 10 ? 2 : 3;
  const text = Number(value.toFixed(digits)).toString();
  return control.unit ? `${text}${control.unit}` : text;
}

/** The controls in the order they are shown: the `ctl`s, then each effect's own parameters under its name. */
export function groups(controls: ControlInfo[]) {
  const visible = controls.filter(control => !control.name.startsWith('__'));
  const plain = visible.filter(control => control.kind === 'ctl');
  const out: { title: string | null; items: ControlInfo[] }[] = [];
  if (plain.length) out.push({ title: null, items: plain });
  const byEffect = new Map<string, ControlInfo[]>();
  for (const control of visible.filter(control => control.kind === 'fx')) {
    const effect = control.name.split('::')[0];
    byEffect.set(effect, [...(byEffect.get(effect) ?? []), control]);
  }
  for (const [title, items] of byEffect) out.push({ title, items });
  return out;
}

/** The part of a control's name to show: an effect's parameters are shown under the effect. */
export const shortName = (control: ControlInfo) => control.name.split('::').pop() ?? control.name;

/** Everything the dock needs to draw. */
export type DockState = { controls: ControlInfo[]; gestures: Clip[] };

export type DockActions = {
  /** Move a control to a value in its own unit. */
  set(name: string, value: number): void;
  /** Start a gesture, as clicking its clip would. */
  launch(clip: string): void;
};

/**
 * The controls dock: a slider for each control that follows the engine (so a knob on a controller moves it),
 * a pad for each gesture, and a held pad for each control bound to a key.
 */
export class Dock {
  private body: HTMLElement;
  private sliders = new Map<string, { input: HTMLInputElement; readout: HTMLElement; control: ControlInfo }>();
  /** The slider being dragged is not moved by the engine's reading until it is let go. */
  private dragging: string | null = null;
  private shape = '';

  constructor(
    readonly element: HTMLElement,
    private actions: DockActions,
  ) {
    this.body = element.querySelector<HTMLElement>('.dock-body')!;
  }

  update(state: DockState) {
    const shape = JSON.stringify([
      state.controls.map(c => [c.name, c.range, c.log, c.source, c.unit]),
      state.gestures.map(c => c.name),
    ]);
    if (shape !== this.shape) {
      this.shape = shape;
      this.build(state);
    }
    for (const control of state.controls) {
      const slider = this.sliders.get(control.name);
      if (!slider) continue;
      slider.control = control;
      slider.readout.textContent = format(control, control.value);
      if (this.dragging !== control.name) slider.input.value = String(toPosition(control, control.value) * 1000);
    }
  }

  private build(state: DockState) {
    this.body.replaceChildren();
    this.sliders.clear();
    if (state.gestures.length || state.controls.some(isGate)) this.body.append(this.pads(state));
    for (const group of groups(state.controls)) {
      const holder = group.title ? document.createElement('details') : document.createElement('div');
      holder.className = 'dock-group';
      if (group.title) {
        const title = document.createElement('summary');
        title.textContent = `${group.title} · ${group.items.length}`;
        holder.append(title);
      }
      const row = document.createElement('div');
      row.className = 'dock-row';
      for (const control of group.items) row.append(this.slider(control));
      holder.append(row);
      this.body.append(holder);
    }
    if (!this.body.childElementCount) {
      const empty = document.createElement('div');
      empty.className = 'dock-empty';
      empty.textContent = 'No controls yet. A `ctl` in the code, or a gesture clip, shows up here.';
      this.body.append(empty);
    }
  }

  private pads(state: DockState) {
    const row = document.createElement('div');
    row.className = 'dock-group dock-pads';
    for (const clip of state.gestures) {
      const pad = document.createElement('button');
      pad.className = 'dock-pad';
      pad.textContent = clip.name;
      pad.title = `Start the gesture ${clip.name}`;
      pad.addEventListener('click', () => this.actions.launch(clip.name));
      row.append(pad);
    }
    for (const control of state.controls.filter(isGate)) {
      const pad = document.createElement('button');
      pad.className = 'dock-pad hold';
      pad.textContent = control.name;
      pad.title = `Hold to open ${control.name}${control.source ? ` (${control.source})` : ''}`;
      const [lo, hi] = span(control);
      pad.addEventListener('pointerdown', event => {
        pad.setPointerCapture(event.pointerId);
        pad.classList.add('down');
        this.actions.set(control.name, hi);
      });
      const release = () => {
        if (!pad.classList.contains('down')) return;
        pad.classList.remove('down');
        this.actions.set(control.name, lo);
      };
      pad.addEventListener('pointerup', release);
      pad.addEventListener('pointercancel', release);
      row.append(pad);
    }
    return row;
  }

  private slider(control: ControlInfo) {
    const cell = document.createElement('label');
    cell.className = 'dock-slider';
    cell.title = control.source ? `${control.name} · ${control.source}` : control.name;
    const name = document.createElement('span');
    name.className = 'dock-name';
    name.textContent = shortName(control);
    const input = document.createElement('input');
    input.type = 'range';
    input.min = '0';
    input.max = '1000';
    input.step = '1';
    input.value = String(toPosition(control, control.value) * 1000);
    const readout = document.createElement('span');
    readout.className = 'dock-value';
    const entry = { input, readout, control };
    let pending: number | null = null;
    this.sliders.set(control.name, entry);
    input.addEventListener('pointerdown', () => (this.dragging = control.name));
    const done = () => (this.dragging = null);
    input.addEventListener('pointerup', done);
    input.addEventListener('pointercancel', done);
    input.addEventListener('input', () => {
      // a key press moves it too, so the first move of a keyboard user holds it for that move
      this.dragging ??= control.name;
      const value = fromPosition(entry.control, Number(input.value) / 1000);
      readout.textContent = format(entry.control, value);
      // a drag can send far more moves than the screen shows: send the latest once a frame
      const waiting = pending !== null;
      pending = value;
      if (waiting) return;
      requestAnimationFrame(() => {
        const latest = pending;
        pending = null;
        if (latest !== null) this.actions.set(control.name, latest);
      });
    });
    input.addEventListener('change', done);
    cell.append(name, input, readout);
    if (control.source) {
      const tag = document.createElement('span');
      tag.className = 'dock-source';
      tag.textContent = control.source.replace('midi.', '').replace('osc_in', 'osc');
      cell.append(tag);
    }
    return cell;
  }
}
