import { builtInEffects } from './effects';
import type { EffectParameters, TrackFx, Lfo, Parameter, Project, FxTarget } from './project';
import { trackColour } from './grid';

export function renderMixer(parent: HTMLElement, project: Project, activeTrack: number, edited: () => void, parameters: EffectParameters = {}, write: (target: FxTarget, value: number) => void = () => {}, loadPreset: (preset: string) => void = () => {}) {
  parent.replaceChildren();
  const track = project.tracks[activeTrack];
  const refresh = () => renderMixer(parent, project, activeTrack, edited, parameters, write, loadPreset);
  const output = document.createElement('section'); output.className = 'device-card track-output'; output.style.setProperty('--track-colour', trackColour(track, activeTrack));
  const heading = document.createElement('div'); heading.className = 'device-heading';
  const name = document.createElement('strong'); name.textContent = `${String(activeTrack + 1).padStart(2, '0')} ${track.name}`; heading.append(name); output.append(heading);
  const volume = slider('Volume', track.volume_db, -60, 6, 'dB', value => { track.volume_db = value; edited(); }, () => write({ kind: 'volume' }, track.volume_db));
  volume.querySelector('input')!.setAttribute('aria-label', `${track.name} volume in dB`); output.append(volume);
  const pan = slider('Pan', track.pan, -1, 1, '', value => { track.pan = value; edited(); }, () => write({ kind: 'pan' }, track.pan));
  pan.querySelector('input')!.setAttribute('aria-label', `${track.name} pan`); output.append(pan); parent.append(output);
  const effect = document.createElement('section'); effect.className = 'device-card effect-device';
  const title = document.createElement('div'); title.className = 'device-heading';
  const select = document.createElement('select'); select.setAttribute('aria-label', 'Track FX preset');
  select.add(new Option('Preset…', ''));
  select.add(new Option('Pass through', 'dry'));
  for (const preset of builtInEffects) select.add(new Option(preset.name, preset.id));
  select.onchange = () => { if (select.value) loadPreset(select.value); };
  const label = document.createElement('span'); label.className = 'section-title'; label.textContent = 'FX';
  title.append(label, select);
  effect.append(title);
  const slot = track.fx;
  const note = (text: string) => { const hint = document.createElement('p'); hint.className = 'device-note'; hint.textContent = text; effect.append(hint); };
  if (slot) {
    const params = parameters[slot.definition];
    if (!params) note('Apply to expose parameters');
    else if (!params.length) note('No parameters');
    else params.forEach(parameter => {
      const value = slot.values[parameter.name] ?? parameter.default;
      const lfo = slot.modulations[parameter.name];
      const toggle = button('∿', `Toggle ${parameter.name} LFO`, () => {
        if (slot.modulations[parameter.name]) delete slot.modulations[parameter.name];
        else slot.modulations[parameter.name] = { enabled: true, shape: 'sine', period: 16, min: 0, max: 1 };
        edited(); refresh();
      }); toggle.setAttribute('aria-pressed', String(!!lfo?.enabled));
      const row = slider(parameter.name, value, parameter.min, parameter.max, parameter.unit ?? '', value => { slot.values[parameter.name] = value; edited(); }, () => {
        write({ kind: 'effect', parameter: parameter.name }, Math.min(parameter.max, Math.max(parameter.min, slot.values[parameter.name] ?? parameter.default)));
      }, toggle);
      row.querySelector('input')!.setAttribute('aria-label', `${track.name} FX ${parameter.name}`); effect.append(row);
      if (lfo) effect.append(lfoControls(slot, parameter, lfo, edited));
    });
  } else note('No FX');
  parent.append(effect);
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
function lfoControls(slot: TrackFx, parameter: Parameter, lfo: Lfo, edited: () => void) {
  const panel = document.createElement('div'); panel.className = 'lfo-controls';
    const shape = document.createElement('select'); shape.setAttribute('aria-label', `${parameter.name} LFO shape`);
  for (const name of ['sine', 'triangle', 'saw', 'square'] as const) shape.add(new Option(name, name)); shape.value = lfo.shape;
  shape.onchange = () => { lfo.shape = shape.value as Lfo['shape']; edited(); }; panel.append(shape);
  for (const [name, field, min, max] of [['Rows', 'period', 1, 256], ['Min%', 'min', 0, 100], ['Max%', 'max', 0, 100]] as const) {
    const label = document.createElement('label'); label.textContent = name; const input = document.createElement('input'); input.type = 'number'; input.min = String(min); input.max = String(max); input.value = String(field === 'period' ? lfo[field] : lfo[field] * 100); input.setAttribute('aria-label', `${parameter.name} LFO ${name}`);
    input.onchange = () => { let value = Math.min(max, Math.max(min, Number(input.value) || min)); if (field !== 'period') value /= 100;
      lfo[field] = field === 'min' ? Math.min(value, lfo.max) : field === 'max' ? Math.max(value, lfo.min) : value; input.value = String(field === 'period' ? lfo[field] : lfo[field] * 100); slot.modulations[parameter.name] = lfo; edited(); };
    label.append(input); panel.append(label);
  }
  return panel;
}
