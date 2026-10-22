import { builtInEffects } from './effects';
import type { EffectParameters, Device, Lfo, Parameter, Project, FxTarget } from './project';
import { maxDevices } from './project';
import { trackColour } from './grid';

/** What the FX view can do to a track's chain; the owner keeps the editor and engine in step. */
export interface FxActions {
  selected: string | null;
  select(id: string): void;
  add(preset: string): void;
  remove(id: string): void;
  move(id: string, delta: number): void;
  changed(): void;
}

export function renderMixer(parent: HTMLElement, project: Project, activeTrack: number, edited: () => void, parameters: EffectParameters = {}, write: (target: FxTarget, value: number) => void = () => {}, fx: FxActions) {
  parent.replaceChildren();
  const track = project.tracks[activeTrack];
  const refresh = () => renderMixer(parent, project, activeTrack, edited, parameters, write, fx);
  const output = document.createElement('section'); output.className = 'device-card track-output'; output.style.setProperty('--track-colour', trackColour(track, activeTrack));
  const heading = document.createElement('div'); heading.className = 'device-heading';
  const name = document.createElement('strong'); name.textContent = `${String(activeTrack + 1).padStart(2, '0')} ${track.name}`; heading.append(name); output.append(heading);
  const volume = slider('Volume', track.volume_db, -60, 6, 'dB', value => { track.volume_db = value; edited(); }, () => write({ kind: 'volume' }, track.volume_db));
  volume.querySelector('input')!.setAttribute('aria-label', `${track.name} volume in dB`); output.append(volume);
  const pan = slider('Pan', track.pan, -1, 1, '', value => { track.pan = value; edited(); }, () => write({ kind: 'pan' }, track.pan));
  pan.querySelector('input')!.setAttribute('aria-label', `${track.name} pan`); output.append(pan); parent.append(output);
  const effects = document.createElement('section'); effects.className = 'device-card effect-device';
  const title = document.createElement('div'); title.className = 'device-heading';
  const label = document.createElement('span'); label.className = 'section-title'; label.textContent = 'FX';
  const add = document.createElement('select'); add.setAttribute('aria-label', 'Add effect');
  add.add(new Option('+ Add effect…', ''));
  add.add(new Option('Blank', 'blank'));
  for (const preset of builtInEffects) add.add(new Option(preset.name, preset.id));
  add.disabled = track.fx.length >= maxDevices;
  add.onchange = () => { if (add.value) fx.add(add.value); };
  title.append(label, add); effects.append(title);
  const note = (text: string) => { const hint = document.createElement('p'); hint.className = 'device-note'; hint.textContent = text; effects.append(hint); };
  if (!track.fx.length) note('No effects');
  track.fx.forEach((device, position) => {
    const definition = project.definitions.find(d => d.id === device.definition);
    const card = document.createElement('div'); card.className = 'device'; card.classList.toggle('selected', device.id === fx.selected); card.classList.toggle('bypassed', device.bypass);
    const head = document.createElement('div'); head.className = 'device-head';
    const pick = button(definition?.name ?? 'Effect', `Edit ${definition?.name ?? 'effect'}`, () => fx.select(device.id)); pick.className = 'device-name';
    const bypass = button('⏻', `Bypass ${definition?.name ?? 'effect'}`, () => { device.bypass = !device.bypass; fx.changed(); }); bypass.setAttribute('aria-pressed', String(device.bypass));
    const up = button('↑', 'Move effect earlier', () => fx.move(device.id, -1)); up.disabled = position === 0;
    const down = button('↓', 'Move effect later', () => fx.move(device.id, 1)); down.disabled = position === track.fx.length - 1;
    const remove = button('×', `Remove ${definition?.name ?? 'effect'}`, () => fx.remove(device.id));
    head.append(pick, bypass, up, down, remove); card.append(head);
    if (device.id === fx.selected) {
      const params = parameters[device.definition];
      const hint = (text: string) => { const p = document.createElement('p'); p.className = 'device-note'; p.textContent = text; card.append(p); };
      if (!params) hint('Apply to expose parameters');
      else if (!params.length) hint('No parameters');
      else params.forEach(parameter => {
        const value = device.values[parameter.name] ?? parameter.default;
        const lfo = device.modulations[parameter.name];
        const toggle = button('∿', `Toggle ${parameter.name} LFO`, () => {
          if (device.modulations[parameter.name]) delete device.modulations[parameter.name];
          else device.modulations[parameter.name] = { enabled: true, shape: 'sine', period: 16, min: 0, max: 1 };
          edited(); refresh();
        }); toggle.setAttribute('aria-pressed', String(!!lfo?.enabled));
        const row = slider(parameter.name, value, parameter.min, parameter.max, parameter.unit ?? '', value => { device.values[parameter.name] = value; edited(); }, () => {
          write({ kind: 'effect', device: device.id, parameter: parameter.name }, Math.min(parameter.max, Math.max(parameter.min, device.values[parameter.name] ?? parameter.default)));
        }, toggle);
        row.querySelector('input')!.setAttribute('aria-label', `${track.name} FX ${parameter.name}`); card.append(row);
        if (lfo) card.append(lfoControls(device, parameter, lfo, edited));
      });
    }
    effects.append(card);
  });
  parent.append(effects);
}

function button(text: string, label: string, click: () => void) { const button = document.createElement('button'); button.textContent = text; button.title = label; button.setAttribute('aria-label', label); button.onclick = click; return button; }
function slider(name: string, initial: number, min: number, max: number, unit: string, change: (value: number) => void, record: () => void, extra?: HTMLElement) {
  const row = document.createElement('label'); row.className = 'parameter-row'; const label = document.createElement('span'); label.textContent = name;
  const input = document.createElement('input'); input.type = 'range'; input.min = String(min); input.max = String(max); input.step = 'any'; input.disabled = max <= min; input.value = String(initial); input.setAttribute('aria-label', name);
  const value = document.createElement('input'); value.type = 'number'; value.className = 'parameter-value'; value.min = String(min); value.max = String(max); value.step = 'any'; value.disabled = max <= min; value.setAttribute('aria-label', `${name} value`);
  const display = () => { value.value = String(Number(Number(input.value).toPrecision(6))); }; display();
  input.oninput = () => { display(); change(Number(input.value)); }; input.oncontextmenu = event => { event.preventDefault(); record(); };
  value.onchange = () => { if (!Number.isFinite(value.valueAsNumber)) { display(); return; } input.value = String(Math.min(max, Math.max(min, value.valueAsNumber))); display(); change(Number(input.value)); };
  const numeric = document.createElement('span'); numeric.className = 'parameter-numeric'; const suffix = document.createElement('small'); suffix.textContent = unit; numeric.append(value, suffix);
  row.append(label, input, numeric, button('↳', `Write ${name} to pattern`, record)); if (extra) row.append(extra); return row;
}
function lfoControls(device: Device, parameter: Parameter, lfo: Lfo, edited: () => void) {
  const panel = document.createElement('div'); panel.className = 'lfo-controls';
    const shape = document.createElement('select'); shape.setAttribute('aria-label', `${parameter.name} LFO shape`);
  for (const name of ['sine', 'triangle', 'saw', 'square'] as const) shape.add(new Option(name, name)); shape.value = lfo.shape;
  shape.onchange = () => { lfo.shape = shape.value as Lfo['shape']; edited(); }; panel.append(shape);
  for (const [name, field, min, max] of [['Rows', 'period', 1, 256], ['Min%', 'min', 0, 100], ['Max%', 'max', 0, 100]] as const) {
    const label = document.createElement('label'); label.textContent = name; const input = document.createElement('input'); input.type = 'number'; input.min = String(min); input.max = String(max); input.value = String(field === 'period' ? lfo[field] : lfo[field] * 100); input.setAttribute('aria-label', `${parameter.name} LFO ${name}`);
    input.onchange = () => { let value = Math.min(max, Math.max(min, Number(input.value) || min)); if (field !== 'period') value /= 100;
      lfo[field] = field === 'min' ? Math.min(value, lfo.max) : field === 'max' ? Math.max(value, lfo.min) : value; input.value = String(field === 'period' ? lfo[field] : lfo[field] * 100); device.modulations[parameter.name] = lfo; edited(); };
    label.append(input); panel.append(label);
  }
  return panel;
}
