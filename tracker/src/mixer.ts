import { builtInEffects, returnIndex, sendPreset, stripOf } from './effects';
import type { EffectParameters, Definition, Device, Lfo, Parameter, Project, FxTarget } from './project';
import { maxDevices } from './project';
import { trackColour, masterColour } from './grid';
import { openBrowser } from './browser';
import { readLfos, setLfo, lfoShapes, modulatable, type SourceLfo } from './modulation';
/** An instrument's parameters as sliders. Notes start with these values; lanes change them from a row on. */
export function renderInstrument(parent: HTMLElement, definition: Definition, parameters: EffectParameters, edited: () => void, write: (target: FxTarget, value: number) => void, activeTrack: number, changeSource: (source: string) => void) {
  parent.replaceChildren();
  const card = document.createElement('section'); card.className = 'device-card instrument-card';
  const heading = document.createElement('div'); heading.className = 'device-heading';
  const label = document.createElement('span'); label.className = 'section-title'; label.textContent = 'INSTRUMENT';
  const name = document.createElement('strong'); name.textContent = definition.name; heading.append(label, name); card.append(heading);
  const params = parameters[definition.id];
  const note = (text: string) => { const hint = document.createElement('p'); hint.className = 'device-note'; hint.textContent = text; card.append(hint); };
  const refresh = () => renderInstrument(parent, definition, parameters, edited, write, activeTrack, changeSource);
  const lfos = readLfos(definition.source);
  const hidden = new Set(definition.hidden ?? []);
  const setHidden = () => { if (hidden.size) definition.hidden = [...hidden]; else delete definition.hidden; edited(); refresh(); };
  if (!params) note('Apply to expose parameters');
  else if (!params.length) note('No parameters');
  else {
    for (const parameter of params.filter(p => !hidden.has(p.name))) {
      const hide = button('−', `Hide the ${parameter.name} slider (its lanes still work)`, () => { hidden.add(parameter.name); setHidden(); });
      hide.setAttribute('aria-label', `Hide ${parameter.name}`);
      const row = slider(parameter.name, definition.values?.[parameter.name] ?? parameter.default, parameter.min, parameter.max, parameter.unit ?? '', value => { (definition.values ??= {})[parameter.name] = value; edited(); }, () => {
        const value = definition.values?.[parameter.name] ?? parameter.default;
        write({ kind: 'instrument', instrument: definition.id, parameter: parameter.name }, Math.min(parameter.max, Math.max(parameter.min, value)));
      }, hide);
      row.querySelector('input')!.setAttribute('aria-label', `${definition.name} ${parameter.name}`);
      const record = row.querySelector<HTMLButtonElement>('button')!;
      record.disabled = activeTrack < 0; if (activeTrack < 0) record.title = 'Only a track has parameter lanes';
      const lfo = lfos[parameter.name];
      const toggle = button('∿', modulatable(parameter) ? `Toggle ${parameter.name} LFO` : `${parameter.name} is a gain in decibels, which an LFO can't move`, () => {
        const source = setLfo(definition.source, parameter, lfo ? null : { shape: 'sine', rate: 2, depth: 0.5 });
        if (source === null) note('This instrument has no code to put an LFO in'); else changeSource(source);
      });
      toggle.disabled = !modulatable(parameter); toggle.setAttribute('aria-pressed', String(!!lfo));
      row.classList.add('with-lfo'); row.append(toggle);
      card.append(row);
      if (lfo) card.append(sourceLfoControls(definition.source, parameter, lfo, changeSource));
    }
    const count = params.filter(p => hidden.has(p.name)).length;
    if (count) {
      const show = document.createElement('button'); show.className = 'show-sliders'; show.textContent = `${count} hidden · show all`; show.onclick = () => { hidden.clear(); setHidden(); };
      card.append(show);
    }
  }
  parent.append(card);
}

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
  const track = stripOf(project, activeTrack), isMaster = activeTrack < 0, isReturn = activeTrack < -1;
  const refresh = () => renderMixer(parent, project, activeTrack, edited, parameters, write, fx);
  const output = document.createElement('section'); output.className = 'device-card track-output'; output.style.setProperty('--track-colour', isMaster ? masterColour : trackColour(track, activeTrack));
  const heading = document.createElement('div'); heading.className = 'device-heading';
  const name = document.createElement('strong'); name.textContent = isMaster ? 'Master' : track.name; heading.append(name); output.append(heading);
  const volume = slider('Volume', track.volume_db, -60, 6, 'dB', value => { track.volume_db = value; edited(); }, () => write({ kind: 'volume' }, track.volume_db));
  volume.querySelector('input')!.setAttribute('aria-label', `${track.name} volume in dB`); output.append(volume);
  const pan = slider('Pan', track.pan, -1, 1, '', value => { track.pan = value; edited(); }, () => write({ kind: 'pan' }, track.pan));
  pan.querySelector('input')!.setAttribute('aria-label', `${track.name} pan`); output.append(pan); parent.append(output);
  const effects = document.createElement('section'); effects.className = 'device-card effect-device';
  const title = document.createElement('div'); title.className = 'device-heading';
  const label = document.createElement('span'); label.className = 'section-title'; label.textContent = 'FX';
  const add = document.createElement('button'); add.textContent = '+ Add effect…'; add.setAttribute('aria-label', 'Add effect');
  add.disabled = track.fx.length >= maxDevices;
  add.onclick = () => {
    const sendAllowed = activeTrack !== -1 && !(isReturn && returnIndex(activeTrack) >= project.returns.length - 1);
    openBrowser(add, [
      ...project.definitions.filter(d => d.kind === 'fx').map(d => ({ value: `fx:${d.id}`, name: d.name, folder: 'Project effects', description: 'An effect already in this project: edits to its code reach every device using it' })),
      { value: 'blank', name: 'Blank', folder: '', description: 'A new effect that passes the sound through, to write yourself' },
      ...(sendAllowed ? [{ value: sendPreset.id, name: sendPreset.name, folder: 'routing', description: 'Sends some of this track to a return' }] : []),
      ...builtInEffects.map(p => ({ value: p.id, name: p.name, folder: p.folder, description: p.description })),
    ], item => fx.add(item.value));
  };
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
    head.append(pick);
    if (device.target !== undefined) {
      const to = document.createElement('select'); to.className = 'send-target'; to.setAttribute('aria-label', `${definition?.name ?? 'Send'} destination`); to.title = 'The return this send feeds';
      project.returns.forEach((r, i) => { if (!isReturn || i > returnIndex(activeTrack)) to.add(new Option(`→ ${r.name}`, String(i))); });
      if (device.target >= project.returns.length) to.add(new Option('→ no return', String(device.target)));
      to.value = String(device.target);
      to.onchange = () => { device.target = Number(to.value); fx.changed(); };
      head.append(to);
    }
    head.append(bypass, up, down, remove); card.append(head);
    if (device.target !== undefined) {
      const options = document.createElement('div'); options.className = 'send-options';
      const flag = (text: string, title: string, key: 'pre' | 'mute_source', disabled = false) => {
        const label = document.createElement('label'); label.title = title; const box = document.createElement('input'); box.type = 'checkbox'; box.checked = !!device[key]; box.disabled = disabled; box.setAttribute('aria-label', title);
        box.onchange = () => { device[key] = box.checked; fx.changed(); }; label.append(box, text); return label;
      };
      options.append(flag('Pre-fader', 'Send before the fader, at this place in the chain', 'pre'), flag('Mute source', 'Silence the chain after this send, so only sends carry the sound on', 'mute_source', !device.pre));
      card.append(options);
    }
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
/** The shape, rate and depth of an LFO that the instrument's code carries; each change rewrites the code. */
function sourceLfoControls(source: string, parameter: Parameter, lfo: SourceLfo, changeSource: (source: string) => void) {
  const panel = document.createElement('div'); panel.className = 'lfo-controls';
  const change = (next: SourceLfo) => { const text = setLfo(source, parameter, next); if (text !== null) changeSource(text); };
  const shape = document.createElement('select'); shape.setAttribute('aria-label', `${parameter.name} LFO shape`);
  for (const name of lfoShapes) shape.add(new Option(name, name)); shape.value = lfo.shape;
  shape.onchange = () => change({ ...lfo, shape: shape.value as SourceLfo['shape'] });
  const field = (name: string, value: number, min: number, max: number, set: (value: number) => SourceLfo) => {
    const label = document.createElement('label'); label.textContent = name;
    const input = document.createElement('input'); input.type = 'number'; input.min = String(min); input.max = String(max); input.step = 'any'; input.value = String(value); input.setAttribute('aria-label', `${parameter.name} LFO ${name}`);
    input.onchange = () => { if (Number.isFinite(input.valueAsNumber)) change(set(Math.min(max, Math.max(min, input.valueAsNumber)))); else input.value = String(value); };
    label.append(input); return label;
  };
  panel.append(shape, field('Hz', lfo.rate, 0.01, 20, rate => ({ ...lfo, rate })), field('Depth %', Math.round(lfo.depth * 100), 1, 100, depth => ({ ...lfo, depth: depth / 100 })));
  return panel;
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
