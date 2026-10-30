import { addMacro, linkParameter, removeMacro, returnIndex, stripOf } from './effects';
import type { EffectParameters, Definition, Device, Lfo, MacroLink, Parameter, Project, FxTarget } from './project';
import { maxMacros } from './project';
import { trackColour, masterColour } from './grid';
import { dropTarget } from './drag';
import { divisions, nearestDivision } from './divisions';
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
  if (!params) note('Parameters appear once the engine has applied this');
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
      record.disabled = activeTrack < 0; if (activeTrack < 0) record.title = 'Only a track has automation lanes';
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
  /** Gives the effect a new name, in its code too. */
  rename(id: string, name: string): void;
  move(id: string, delta: number): void;
  changed(): void;
  /** Lanes were deleted from the pattern grid, so what it remembers of them is out of date. */
  lanesRemoved(): void;
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
  pan.querySelector('input')!.setAttribute('aria-label', `${track.name} pan`); output.append(pan);
  const channel = document.createElement('div'); channel.className = 'mixer-channel';
  channel.append(output, macrosCard(project, activeTrack, edited, write, refresh, fx.lanesRemoved)); parent.append(channel);
  const before = parent.querySelector<HTMLElement>('.effect-device'), scrolled = before?.scrollLeft ?? 0, selectedBefore = before?.dataset.selected;
  const effects = document.createElement('section'); effects.className = 'effect-device'; effects.setAttribute('aria-label', 'Effect chain'); effects.dataset.selected = fx.selected ?? '';
  effects.addEventListener('wheel', event => wheelAlong(effects, event), { passive: false });
  dropTarget(effects, item => item.kind === 'fx', item => fx.add(item.value));
  if (!track.fx.length) { const hint = document.createElement('p'); hint.className = 'device-note'; hint.textContent = 'No effects yet: double-click one in the list, or drag it here'; effects.append(hint); }
  track.fx.forEach((device, position) => {
    const definition = project.definitions.find(d => d.id === device.definition);
    const card = document.createElement('div'); card.className = 'device'; card.classList.toggle('selected', device.id === fx.selected); card.classList.toggle('bypassed', device.bypass);
    const head = document.createElement('div'); head.className = 'device-head';
    const pick = button(definition?.name ?? 'Effect', `Edit ${definition?.name ?? 'effect'}`, () => { if (device.id !== fx.selected) fx.select(device.id); }); pick.className = 'device-name'; pick.title = 'Click to select, double-click to rename, drag to reorder';
    if (definition) pick.ondblclick = () => renameInPlace(pick, definition.name, name => fx.rename(device.id, name));
    const bypass = button('⏻', `Bypass ${definition?.name ?? 'effect'}`, () => { device.bypass = !device.bypass; fx.changed(); }); bypass.setAttribute('aria-pressed', String(device.bypass));
    const up = button('←', 'Move effect earlier', () => fx.move(device.id, -1)); up.disabled = position === 0;
    const down = button('→', 'Move effect later', () => fx.move(device.id, 1)); down.disabled = position === track.fx.length - 1;
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
      options.append(flag('Pre-fader', 'Send before the fader, at this place in the chain', 'pre'), flag('Mute source', device.pre ? 'Silence the chain after this send, so only sends carry the sound on' : 'Silence this track itself, so only its sends are heard (they still follow the fader)', 'mute_source'));
      card.append(options);
    }
    {
      const params = parameters[device.definition];
      const hint = (text: string) => { const p = document.createElement('p'); p.className = 'device-note'; p.textContent = text; card.append(p); };
      if (!params) hint('Parameters appear once the engine has applied this');
      else if (!params.length) hint('No parameters');
      else params.forEach(parameter => {
        const value = device.values[parameter.name] ?? parameter.default;
        const lfo = device.modulations[parameter.name];
        const toggle = button('∿', `Toggle ${parameter.name} LFO`, () => {
          if (device.modulations[parameter.name]) delete device.modulations[parameter.name];
          else device.modulations[parameter.name] = { enabled: true, shape: 'sine', period: 16, min: 0, max: 1 };
          edited(); refresh();
        }); toggle.setAttribute('aria-pressed', String(!!lfo?.enabled));
        const macros = track.macros ?? [];
        const driver = macros.findIndex(knob => knob.links.some(link => link.device === device.id && link.parameter === parameter.name));
        const link = driver < 0 ? undefined : macros[driver].links.find(link => link.device === device.id && link.parameter === parameter.name);
        const pick = document.createElement('select'); pick.className = 'macro-link'; pick.setAttribute('aria-label', `${parameter.name} macro`);
        pick.title = macros.length ? 'The macro that moves this parameter' : 'Add a macro to drive this parameter from';
        pick.add(new Option('—', '')); macros.forEach((knob, i) => pick.add(new Option(String(i + 1), String(i)))); pick.value = driver < 0 ? '' : String(driver); pick.disabled = !macros.length;
        pick.onchange = () => { linkParameter(project, activeTrack, device.id, parameter.name, pick.value === '' ? null : Number(pick.value)); edited(); refresh(); };
        const extras = document.createElement('span'); extras.className = 'row-extras'; extras.append(toggle, pick);
        const change = (value: number) => { device.values[parameter.name] = value; edited(); };
        const record = () => write({ kind: 'effect', device: device.id, parameter: parameter.name }, Math.min(parameter.max, Math.max(parameter.min, device.values[parameter.name] ?? parameter.default)));
        const row = parameter.division ? divisionRow(parameter.name, value, change, record, extras) : slider(parameter.name, value, parameter.min, parameter.max, parameter.unit ?? '', change, record, extras);
        row.querySelector('input, select')!.setAttribute('aria-label', `${track.name} FX ${parameter.name}`); row.classList.add('with-macro'); card.append(row);
        if (link) {
          // the macro sets the parameter, so its own slider, lanes and LFO have nothing to say
          row.classList.add('driven'); row.querySelectorAll('input, select:not(.macro-link), button:not(.macro-link)').forEach(control => { if (control.parentElement !== extras) (control as HTMLInputElement).disabled = true; });
          toggle.disabled = true; toggle.title = 'A macro drives this parameter';
          card.append(macroLinkControls(link, parameter, edited));
        } else if (lfo) card.append(lfoControls(device, parameter, lfo, edited));
      });
    }
    reorderable(pick, card, effects, () => position, to => fx.move(device.id, to - position));
    // a click on the card's own space (not a control) picks it
    card.addEventListener('click', event => { if (device.id !== fx.selected && !(event.target as Element).closest('input, select, button, label')) fx.select(device.id); });
    effects.append(card);
  });
  parent.append(effects);
  effects.scrollLeft = scrolled;
  if (fx.selected && fx.selected !== selectedBefore) effects.querySelector<HTMLElement>('.device.selected')?.scrollIntoView?.({ inline: 'nearest', block: 'nearest' });
}

/** Swaps `label` for a text field holding `name` until Enter or a click elsewhere (which keep the change) or Escape (which drops it). */
function renameInPlace(label: HTMLElement, name: string, commit: (name: string) => void) {
  const input = document.createElement('input'); input.className = 'device-rename'; input.value = name; input.maxLength = 64; input.setAttribute('aria-label', `Rename ${name}`);
  let finished = false;
  const finish = (keep: boolean) => {
    if (finished) return; finished = true;
    const value = input.value.trim();
    if (keep && value && value !== name) commit(value); else input.replaceWith(label);
  };
  input.onkeydown = event => { event.stopPropagation(); if (event.key === 'Enter' || event.key === 'Escape') { event.preventDefault(); finish(event.key === 'Enter'); } };
  input.onblur = () => finish(true);
  label.replaceWith(input); input.focus(); input.select();
}

/** The chain scrolls sideways under a plain wheel, unless the effect under the pointer has more to show in the direction it was turned. */
function wheelAlong(chain: HTMLElement, event: WheelEvent) {
  if (Math.abs(event.deltaX) > Math.abs(event.deltaY)) return;
  const card = (event.target as Element).closest<HTMLElement>('.device');
  if (card && (event.deltaY < 0 ? card.scrollTop > 0 : card.scrollTop + card.clientHeight < card.scrollHeight - 1)) return;
  chain.scrollLeft += event.deltaY; event.preventDefault();
}

/** Dragging an effect's name along the chain: the card under the pointer is marked, and letting go moves the effect to its place. */
function reorderable(handle: HTMLElement, card: HTMLElement, chain: HTMLElement, position: () => number, move: (to: number) => void) {
  handle.addEventListener('pointerdown', down => {
    if (down.button !== 0) return;
    let moved = false, over: HTMLElement | null = null;
    const cards = () => [...chain.querySelectorAll<HTMLElement>(':scope > .device')];
    const track = (event: PointerEvent) => {
      if (!moved && Math.hypot(event.clientX - down.clientX, event.clientY - down.clientY) < 5) return;
      if (!moved) { moved = true; card.classList.add('reordering'); document.body.classList.add('dragging'); }
      const next = cards().find(other => { const box = other.getBoundingClientRect(); return event.clientX >= box.left && event.clientX < box.right; }) ?? null;
      if (next !== over) { over?.classList.remove('drop-over'); over = next === card ? null : next; over?.classList.add('drop-over'); }
    };
    const end = (event: PointerEvent) => {
      document.removeEventListener('pointermove', track, true); document.removeEventListener('pointerup', end, true); document.removeEventListener('pointercancel', end, true);
      if (!moved) return;
      card.classList.remove('reordering'); over?.classList.remove('drop-over'); document.body.classList.remove('dragging');
      const swallow = (click: Event) => { click.stopPropagation(); click.preventDefault(); };
      document.addEventListener('click', swallow, true); setTimeout(() => document.removeEventListener('click', swallow, true));
      if (event.type === 'pointerup' && over) move(cards().indexOf(over));
    };
    document.addEventListener('pointermove', track, true); document.addEventListener('pointerup', end, true); document.addEventListener('pointercancel', end, true);
  });
}

/** The macro knobs of a strip: a slider each, named, that the effect parameters below can follow. */
function macrosCard(project: Project, activeTrack: number, edited: () => void, write: (target: FxTarget, value: number) => void, refresh: () => void, lanesRemoved: () => void) {
  const track = stripOf(project, activeTrack), macros = track.macros ?? [];
  const card = document.createElement('section'); card.className = 'device-card macro-device';
  const title = document.createElement('div'); title.className = 'device-heading';
  const label = document.createElement('span'); label.className = 'section-title'; label.textContent = 'MACROS';
  const add = button('+ Add macro', 'Add macro', () => { if (addMacro(project, activeTrack)) { edited(); refresh(); } });
  add.disabled = macros.length >= maxMacros; title.append(label, add); card.append(title);
  if (!macros.length) { const hint = document.createElement('p'); hint.className = 'device-note'; hint.textContent = 'A macro moves several effect parameters at once'; card.append(hint); }
  macros.forEach((knob, index) => {
    const row = slider(knob.name, knob.value, 0, 1, '', value => { knob.value = value; edited(); }, () => write({ kind: 'macro', index }, knob.value),
      button('×', `Remove macro ${index + 1}: later ones move up a number, in lanes and commands too`, () => { if (removeMacro(project, activeTrack, index)) lanesRemoved(); edited(); refresh(); }));
    const name = document.createElement('input'); name.className = 'macro-name'; name.value = knob.name; name.maxLength = 40; name.setAttribute('aria-label', `Macro ${index + 1} name`);
    name.onchange = () => { knob.name = name.value.trim(); edited(); };
    row.firstElementChild!.replaceWith(name);
    row.querySelector('input[type=range]')!.setAttribute('aria-label', `${track.name} macro ${index + 1}`);
    card.append(row);
  });
  return card;
}

/** How far a macro moves a parameter: where its range sits at the knob's two ends. */
function macroLinkControls(link: MacroLink, parameter: Parameter, edited: () => void) {
  const panel = document.createElement('div'); panel.className = 'lfo-controls macro-controls';
  for (const [name, field] of [['At 0%', 'min'], ['At 100%', 'max']] as const) {
    const label = document.createElement('label'); label.textContent = name;
    const input = document.createElement('input'); input.type = 'number'; input.min = '0'; input.max = '100'; input.step = 'any'; input.value = String(Number((link[field] * 100).toPrecision(4))); input.setAttribute('aria-label', `${parameter.name} macro ${name}`);
    input.onchange = () => { if (Number.isFinite(input.valueAsNumber)) link[field] = Math.min(100, Math.max(0, input.valueAsNumber)) / 100; input.value = String(Number((link[field] * 100).toPrecision(4))); edited(); };
    label.append(input); panel.append(label);
  }
  return panel;
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
  row.append(label, input, numeric, button('↳', `Record ${name} at the cursor row, as automation`, record)); if (extra) row.append(extra); return row;
}
/** A length picked from the musical divisions (1/8, 1/8t, 1/8d...), held in beats so it follows the tempo. */
function divisionRow(name: string, beats: number, change: (value: number) => void, record: () => void, extra?: HTMLElement) {
  const row = document.createElement('label'); row.className = 'parameter-row'; const label = document.createElement('span'); label.textContent = name;
  const pick = document.createElement('select'); pick.className = 'division-pick'; pick.setAttribute('aria-label', name);
  for (const d of divisions) pick.add(new Option(d.name, String(d.beats)));
  pick.value = String(nearestDivision(beats).beats);
  pick.onchange = () => change(Number(pick.value)); pick.oncontextmenu = event => { event.preventDefault(); record(); };
  row.append(label, pick, button('↳', `Record ${name} at the cursor row, as automation`, record)); if (extra) row.append(extra); return row;
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
