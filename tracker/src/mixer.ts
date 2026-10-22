import { remapEffectLanes } from './automation';
import { builtInEffects, copyBuiltInEffect } from './effects';
import type { EffectParameters, EffectSlot, Lfo, Parameter, Project, FxTarget } from './project';
import { trackColour } from './grid';

export function renderMixer(parent: HTMLElement, project: Project, activeTrack: number, selectTrack: (track: number) => void, edited: () => void, editEffect: (id: string) => void, parameters: EffectParameters = {}, write: (target: FxTarget, value: number) => void = () => {}, playing = false, createEffect: () => void = () => {}) {
  parent.replaceChildren();
  const track = project.tracks[activeTrack];
  const refresh = () => renderMixer(parent, project, activeTrack, selectTrack, edited, editEffect, parameters, write, playing, createEffect);
  const output = document.createElement('section'); output.className = 'device-card track-output'; output.style.setProperty('--track-colour', trackColour(activeTrack));
  const heading = document.createElement('div'); heading.className = 'device-heading';
  const picker = document.createElement('select'); picker.setAttribute('aria-label', 'Effects track');
  project.tracks.forEach((t, i) => picker.add(new Option(`${String(i + 1).padStart(2, '0')} ${t.name}`, String(i))));
  picker.value = String(activeTrack); picker.onchange = () => selectTrack(Number(picker.value));
  const mute = button('Mute', `Mute ${track.name}`, () => { track.muted = !track.muted; mute.setAttribute('aria-pressed', String(track.muted)); edited(); }); mute.setAttribute('aria-pressed', String(track.muted)); heading.append(picker, mute); output.append(heading);
  const volume = slider('Pre volume', track.volume_db, -60, 6, 'dB', value => { track.volume_db = value; edited(); }, () => write({ kind: 'volume' }, track.volume_db));
  volume.querySelector('input')!.setAttribute('aria-label', `${track.name} volume in dB`); output.append(volume);
  const pan = slider('Pan', track.pan, -1, 1, '', value => { track.pan = value; edited(); }, () => write({ kind: 'pan' }, track.pan));
  pan.querySelector('input')!.setAttribute('aria-label', `${track.name} pan`); output.append(pan);
  const routing = document.createElement('p'); routing.className = 'device-routing'; routing.textContent = 'Niminal node → insert chain → master'; output.append(routing); parent.append(output);
  track.effects.forEach((slot, index) => {
    const effect = document.createElement('section'); effect.className = 'device-card effect-device';
    const title = document.createElement('div'); title.className = 'device-heading';
    const caption = document.createElement('strong'); caption.textContent = `${index + 1} / ${project.definitions.find(d => d.id === slot.definition)?.name ?? 'Effect'}`; title.append(caption);
    for (const [text, label, delta] of [['←', 'Move effect left', -1], ['→', 'Move effect right', 1]] as const) {
      const control = button(text, `${label} ${index + 1}`, () => { const next = index + delta; remapEffectLanes(project, activeTrack, d => d === index ? next : d === next ? index : d); [track.effects[index], track.effects[next]] = [track.effects[next], track.effects[index]]; edited(); refresh(); });
      control.dataset.structure = ''; control.disabled = playing || index + delta < 0 || index + delta >= track.effects.length; title.append(control);
    }
    const remove = button('×', `Remove effect ${index + 1}`, () => { remapEffectLanes(project, activeTrack, d => d === index ? null : d > index ? d - 1 : d); track.effects.splice(index, 1); edited(); refresh(); }); remove.dataset.structure = ''; remove.disabled = playing; title.append(remove); effect.append(title);
    const select = document.createElement('select'); select.setAttribute('aria-label', `${track.name} effect ${index + 1}`); select.dataset.structure = ''; select.disabled = playing;
    for (const definition of project.definitions.filter(d => d.kind === 'fx')) select.add(new Option(definition.name, definition.id)); select.value = slot.definition;
    select.onchange = () => { remapEffectLanes(project, activeTrack, d => d === index ? null : d); track.effects[index] = { definition: select.value, values: {}, modulations: {} }; edited(); refresh(); }; effect.append(select);
    effect.append(button('{} Edit', `Edit ${track.name} effect ${index + 1}`, () => editEffect(slot.definition)));
    const params = parameters[slot.definition];
    if (!params) { const hint = document.createElement('p'); hint.textContent = 'Connect and Apply this effect to expose its parameters.'; effect.append(hint); }
    else if (!params.length) { const hint = document.createElement('p'); hint.textContent = 'Add parameters to the fx signature to automate it.'; effect.append(hint); }
    else params.forEach(parameter => {
      const value = slot.values[parameter.name] ?? parameter.default;
      const row = slider(parameter.name, value, parameter.min, parameter.max, parameter.unit ?? '', value => { slot.values[parameter.name] = value; edited(); }, () => {
        write({ kind: 'effect', device: index, parameter: parameter.name }, slot.values[parameter.name] ?? parameter.default);
      });
      row.querySelector('input')!.setAttribute('aria-label', `${track.name} effect ${index + 1} ${parameter.name}`); effect.append(row);
      const lfo = slot.modulations[parameter.name];
      const toggle = button('∿ LFO', `Toggle ${parameter.name} LFO on effect ${index + 1}`, () => {
        if (slot.modulations[parameter.name]) delete slot.modulations[parameter.name];
        else slot.modulations[parameter.name] = { enabled: true, shape: 'sine', period: 16, min: 0, max: 1 };
        edited(); refresh();
      }); toggle.setAttribute('aria-pressed', String(!!lfo?.enabled)); effect.append(toggle);
      if (lfo) effect.append(lfoControls(slot, parameter, lfo, edited));
    }); parent.append(effect);
  });
  const add = document.createElement('section'); add.className = 'device-card add-device';
  const select = document.createElement('select'); select.setAttribute('aria-label', 'Effect to add');
  const builtins = document.createElement('optgroup'); builtins.label = 'Built-in effects';
  for (const preset of builtInEffects) builtins.append(new Option(preset.name, `builtin:${preset.id}`));
  const saved = document.createElement('optgroup'); saved.label = 'Project effects';
  for (const definition of project.definitions.filter(d => d.kind === 'fx')) saved.append(new Option(definition.name, definition.id));
  select.append(builtins, saved);
  const append = button('+ Insert effect', 'Add effect to chain', () => {
    if (!select.value || track.effects.length >= 8) return;
    const id = select.value.startsWith('builtin:') ? copyBuiltInEffect(project, select.value.slice(8)).id : select.value;
    track.effects.push({ definition: id, values: {}, modulations: {} }); edited(); refresh();
  }); append.dataset.structure = ''; append.disabled = playing || track.effects.length >= 8;
  const custom = button('+ Custom effect', 'Create and insert custom effect', createEffect); custom.dataset.structure = ''; custom.disabled = playing || track.effects.length >= 8;
  const hint = document.createElement('p'); hint.textContent = 'Choose a parameter in a pattern lane.\n↳ writes a value into its matching lane.';
  add.append(select, append, custom, hint); parent.append(add);
}

function button(text: string, label: string, click: () => void) { const button = document.createElement('button'); button.textContent = text; button.title = label; button.setAttribute('aria-label', label); button.onclick = click; return button; }
function slider(name: string, initial: number, min: number, max: number, unit: string, change: (value: number) => void, record: () => void) {
  const row = document.createElement('label'); row.className = 'parameter-row'; const label = document.createElement('span'); label.textContent = name;
  const input = document.createElement('input'); input.type = 'range'; input.min = String(min); input.max = String(max); input.step = 'any'; input.disabled = max <= min; input.value = String(initial); input.setAttribute('aria-label', name);
  const value = document.createElement('input'); value.type = 'number'; value.className = 'parameter-value'; value.min = String(min); value.max = String(max); value.step = 'any'; value.disabled = max <= min; value.setAttribute('aria-label', `${name} value`);
  const display = () => { value.value = String(Number(Number(input.value).toPrecision(6))); }; display();
  input.oninput = () => { display(); change(Number(input.value)); }; input.oncontextmenu = event => { event.preventDefault(); record(); };
  value.onchange = () => { if (!Number.isFinite(value.valueAsNumber)) { display(); return; } input.value = String(Math.min(max, Math.max(min, value.valueAsNumber))); display(); change(Number(input.value)); };
  const numeric = document.createElement('span'); numeric.className = 'parameter-numeric'; const suffix = document.createElement('small'); suffix.textContent = unit; numeric.append(value, suffix);
  row.append(label, input, numeric, button('↳', `Write ${name} to pattern`, record)); return row;
}
function lfoControls(slot: EffectSlot, parameter: Parameter, lfo: Lfo, edited: () => void) {
  const panel = document.createElement('div'); panel.className = 'lfo-controls';
  const title = document.createElement('small'); title.textContent = 'Niminal LFO → ' + parameter.name; panel.append(title);
  const shape = document.createElement('select'); shape.setAttribute('aria-label', `${parameter.name} LFO shape`);
  for (const name of ['sine', 'triangle', 'saw', 'square'] as const) shape.add(new Option(name, name)); shape.value = lfo.shape;
  shape.onchange = () => { lfo.shape = shape.value as Lfo['shape']; edited(); }; panel.append(shape);
  for (const [name, field, min, max] of [['Lines/cycle', 'period', 1, 256], ['Min %', 'min', 0, 100], ['Max %', 'max', 0, 100]] as const) {
    const label = document.createElement('label'); label.textContent = name; const input = document.createElement('input'); input.type = 'number'; input.min = String(min); input.max = String(max); input.value = String(field === 'period' ? lfo[field] : lfo[field] * 100); input.setAttribute('aria-label', `${parameter.name} LFO ${name}`);
    input.onchange = () => { let value = Math.min(max, Math.max(min, Number(input.value) || min)); if (field !== 'period') value /= 100;
      lfo[field] = field === 'min' ? Math.min(value, lfo.max) : field === 'max' ? Math.max(value, lfo.min) : value; input.value = String(field === 'period' ? lfo[field] : lfo[field] * 100); slot.modulations[parameter.name] = lfo; edited(); };
    label.append(input); panel.append(label);
  }
  return panel;
}
