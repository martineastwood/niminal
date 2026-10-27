// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { renderMixer, renderInstrument, type FxActions } from './mixer';
import { newProject, readProject } from './project';
import { addDevice, addMacro, commandsFor, linkParameter, removeMacro, duplicateEffect, effectUsers, moveDevice, newEffect, removeDevice, removeEffect } from './effects';

vi.stubGlobal('Option', function (text: string, value: string) {
  const option = document.createElement('option'); option.textContent = text; option.value = value; return option;
});
afterEach(() => document.body.replaceChildren());

function actions(overrides: Partial<FxActions> = {}): FxActions {
  return { selected: null, select: vi.fn(), add: vi.fn(), remove: vi.fn(), move: vi.fn(), changed: vi.fn(), lanesRemoved: vi.fn(), ...overrides };
}

/** The names of the presets the Add effect browser lists (not the project's own effects), which it then closes. */
function browsable(parent: HTMLElement) {
  parent.querySelector<HTMLButtonElement>('[aria-label="Add effect"]')!.click();
  const names = [...document.querySelectorAll('.browser-item')].filter(item => !item.closest('details')?.querySelector('summary')?.textContent?.startsWith('Project effects')).map(item => item.textContent);
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
  return names;
}

it('builds a chain from presets, each device with its own copy of the code', () => {
  const project = newProject();
  const a = addDevice(project, 0, 'echo')!, b = addDevice(project, 0, 'echo')!, c = addDevice(project, 1, 'ensemble')!;
  expect(a.definition.id).not.toBe(b.definition.id);
  expect(project.tracks[0].fx.map(d => d.id)).toEqual([a.device.id, b.device.id]);
  expect([a.definition.name, b.definition.name]).toEqual(['Echo', 'Echo 2']);
  expect(a.definition.source).toContain('fx echo('); expect(b.definition.source).toContain('fx echo_2('); expect(a.definition.source).toContain('in.delay');
  expect(c.definition.source).toContain('in.chorus');
  expect(moveDevice(project, 0, b.device.id, -1)).toBe(true);
  expect(project.tracks[0].fx.map(d => d.id)).toEqual([b.device.id, a.device.id]);
  expect(moveDevice(project, 0, b.device.id, -1)).toBe(false);
  const lane = project.patterns[0].fx[0][0]; lane.target = { kind: 'effect', device: a.device.id, parameter: 'mix' };
  const kept = project.patterns[0].fx[0][0];
  expect(removeDevice(project, 0, a.device.id)!.id).toBe(a.device.id);
  expect(project.definitions.some(d => d.id === a.definition.id)).toBe(true); // the effect stays in the library
  expect(kept.target).toBeNull();
  for (let i = 0; i < 10; i++) addDevice(project, 2, 'band_cut');
  expect(project.tracks[2].fx).toHaveLength(8);
});

it('adds sends to tracks only, targeting the first return, and picks the return on the device', () => {
  const project = newProject(); project.returns.push({ name: 'Verb', fx: [], pan: 0, muted: false, volume_db: 0 }, { name: 'Echo', fx: [], pan: 0, muted: false, volume_db: 0 });
  const { device, definition } = addDevice(project, 0, 'send')!;
  expect(device.target).toBe(0); expect(definition.source).toContain('amount'); expect(addDevice(project, 0, 'echo')!.device.target).toBeUndefined();
  const parent = document.createElement('div'), fx = actions({ selected: device.id }); document.body.append(parent);
  renderMixer(parent, project, 0, vi.fn(), {}, vi.fn(), fx);
  expect([...parent.querySelectorAll<HTMLOptionElement>('[aria-label="Send destination"] option')].map(o => o.text)).toEqual(['→ Verb', '→ Echo']);
  const to = parent.querySelector<HTMLSelectElement>('[aria-label="Send destination"]')!; to.value = '1'; to.dispatchEvent(new Event('change'));
  expect(device.target).toBe(1); expect(fx.changed).toHaveBeenCalled();
  const [pre, mute] = [...parent.querySelectorAll<HTMLInputElement>('.send-options input')]; expect(pre.checked).toBe(false); expect(mute.disabled).toBe(true);
  pre.checked = true; pre.dispatchEvent(new Event('change')); expect(device.pre).toBe(true);
  renderMixer(parent, project, 0, vi.fn(), {}, vi.fn(), fx);
  const unlocked = parent.querySelectorAll<HTMLInputElement>('.send-options input')[1]; expect(unlocked.disabled).toBe(false);
  unlocked.checked = true; unlocked.dispatchEvent(new Event('change')); expect(device.mute_source).toBe(true);
  // A return's send starts on the next return and may only pick later ones; the last return and the master cannot send.
  const presets = (strip: number) => { renderMixer(parent, project, strip, vi.fn(), {}, vi.fn(), fx); return browsable(parent); };
  expect(presets(-2)).toContain('Send'); expect(presets(-3)).not.toContain('Send');
  const onReturn = addDevice(project, -2, 'send')!.device; expect(onReturn.target).toBe(1);
  renderMixer(parent, project, -2, vi.fn(), {}, vi.fn(), actions({ selected: onReturn.id }));
  expect([...parent.querySelectorAll<HTMLOptionElement>('.send-target option')].map(o => o.text)).toEqual(['→ Echo']);
  renderMixer(parent, project, -1, vi.fn(), {}, vi.fn(), fx);
  expect(browsable(parent)).not.toContain('Send');
});

it('keeps lanes that hold values when their effect is removed', () => {
  const project = newProject(); const { device } = addDevice(project, 0, 'echo')!;
  const lane = project.patterns[0].fx[0][0]; lane.target = { kind: 'effect', device: device.id, parameter: 'mix' }; lane.envelope = { enabled: true, mode: 'hold', points: [{ row: 4, value: 0.5 }] };
  removeDevice(project, 0, device.id);
  expect(lane.target).toEqual({ kind: 'effect', device: device.id, parameter: 'mix' });
});

it('uses the same editable controls and command recording for custom and built-in parameters', () => {
  const project = newProject(), parent = document.createElement('div'), edited = vi.fn(), write = vi.fn(); document.body.append(parent);
  const { device, definition } = addDevice(project, 0, 'ensemble')!;
  renderMixer(parent, project, 0, edited, { [definition.id]: [{ name: 'mix', min: 0, max: 1, default: 0.4, unit: null }] }, write, actions({ selected: device.id }));
  const value = parent.querySelector<HTMLInputElement>('[aria-label="mix value"]')!;
  value.value = '0.8'; value.dispatchEvent(new Event('change'));
  expect(device.values.mix).toBe(0.8); expect(edited).toHaveBeenCalledOnce();
  parent.querySelector<HTMLButtonElement>('[aria-label="Write mix to pattern"]')!.click(); expect(write).toHaveBeenCalledWith({ kind: 'effect', device: device.id, parameter: 'mix' }, 0.8);
  value.value = '3'; value.dispatchEvent(new Event('change')); expect(device.values.mix).toBe(1);
});

it('shows each device as a card whose buttons act on the chain, with only the selected one expanded', () => {
  const project = newProject(), parent = document.createElement('div'); document.body.append(parent);
  const a = addDevice(project, 0, 'echo')!, b = addDevice(project, 0, 'lowpass')!;
  const fx = actions({ selected: b.device.id });
  renderMixer(parent, project, 0, vi.fn(), { [a.definition.id]: [{ name: 'mix', min: 0, max: 1, default: 0.3, unit: null }], [b.definition.id]: [{ name: 'cutoff', min: 20, max: 20000, default: 2400, unit: 'hz' }] }, vi.fn(), fx);
  expect(parent.querySelectorAll('.device')).toHaveLength(2);
  expect(parent.querySelector('[aria-label="mix value"]')).toBeNull(); expect(parent.querySelector('[aria-label="cutoff value"]')).not.toBeNull();
  parent.querySelector<HTMLButtonElement>('[aria-label="Edit Echo"]')!.click(); expect(fx.select).toHaveBeenCalledWith(a.device.id);
  parent.querySelector<HTMLButtonElement>('[aria-label="Bypass Echo"]')!.click(); expect(a.device.bypass).toBe(true); expect(fx.changed).toHaveBeenCalled();
  parent.querySelector<HTMLButtonElement>('[aria-label="Move effect later"]')!.click(); expect(fx.move).toHaveBeenCalledWith(a.device.id, 1);
  parent.querySelector<HTMLButtonElement>('[aria-label="Remove Lowpass"]')!.click(); expect(fx.remove).toHaveBeenCalledWith(b.device.id);
  expect(parent.querySelector<HTMLButtonElement>('[aria-label="Move effect earlier"]')!.disabled).toBe(true);
  parent.querySelector<HTMLButtonElement>('[aria-label="Add effect"]')!.click();
  [...document.querySelectorAll<HTMLButtonElement>('.browser-item')].find(b => b.textContent === 'Band cut')!.click(); expect(fx.add).toHaveBeenCalledWith('band_cut');
  expect(document.querySelector('.browser')).toBeNull();
});

it('shows an instrument\'s parameters as sliders that set its values, and records them to a lane on a track', () => {
  const project = newProject(), parent = document.createElement('div'), edited = vi.fn(), write = vi.fn(); document.body.append(parent);
  const tone = project.definitions[0];
  const parameters = { tone: [{ name: 'cutoff', min: 200, max: 8000, default: 2000, unit: 'hz' }] };
  renderInstrument(parent, tone, {}, edited, write, 0, vi.fn()); expect(parent.textContent).toContain('Parameters appear once the engine has applied this');
  renderInstrument(parent, tone, parameters, edited, write, 0, vi.fn());
  const value = parent.querySelector<HTMLInputElement>('[aria-label="cutoff value"]')!;
  expect(value.value).toBe('2000'); value.value = '500'; value.dispatchEvent(new Event('change'));
  expect(tone.values).toEqual({ cutoff: 500 }); expect(edited).toHaveBeenCalledOnce();
  parent.querySelector<HTMLButtonElement>('[aria-label="Write cutoff to pattern"]')!.click();
  expect(write).toHaveBeenCalledWith({ kind: 'instrument', instrument: 'tone', parameter: 'cutoff' }, 500);
  renderInstrument(parent, tone, parameters, edited, write, -1, vi.fn());
  expect(parent.querySelector<HTMLButtonElement>('[aria-label="Write cutoff to pattern"]')!.disabled).toBe(true);
});

it('hides an instrument slider on request, and brings all of them back', () => {
  const project = newProject(), parent = document.createElement('div'), edited = vi.fn(); document.body.append(parent);
  const tone = project.definitions[0];
  const parameters = { tone: [{ name: 'cutoff', min: 200, max: 8000, default: 2000, unit: 'hz' }, { name: 'amp', min: 0, max: 1, default: 0.5, unit: null }] };
  renderInstrument(parent, tone, parameters, edited, vi.fn(), 0, vi.fn());
  parent.querySelector<HTMLButtonElement>('[aria-label="Hide cutoff"]')!.click();
  expect(tone.hidden).toEqual(['cutoff']); expect(edited).toHaveBeenCalledOnce();
  expect(parent.querySelector('[aria-label="cutoff value"]')).toBeNull(); expect(parent.querySelector('[aria-label="amp value"]')).not.toBeNull();
  expect(readProject(JSON.stringify(project)).definitions[0].hidden).toEqual(['cutoff']);
  parent.querySelector<HTMLButtonElement>('.show-sliders')!.click();
  expect(tone.hidden).toBeUndefined(); expect(parent.querySelector('[aria-label="cutoff value"]')).not.toBeNull(); expect(parent.querySelector('.show-sliders')).toBeNull();
});

it('keeps effects in a library that devices on any track, or the master, can reuse', () => {
  const project = newProject();
  const delay = newEffect(project, 'echo')!, again = newEffect(project, 'echo')!;
  expect([delay.name, again.name]).toEqual(['Echo', 'Echo 2']);
  const onLead = addDevice(project, 0, `fx:${delay.id}`)!, onBass = addDevice(project, 1, `fx:${delay.id}`)!, onMaster = addDevice(project, -1, `fx:${delay.id}`)!;
  expect(new Set([onLead.definition.id, onBass.definition.id, onMaster.definition.id])).toEqual(new Set([delay.id]));
  expect(new Set([onLead.device.id, onBass.device.id, onMaster.device.id]).size).toBe(3);
  expect(project.master.fx).toHaveLength(1);
  expect(effectUsers(project, delay.id)).toEqual(['Master', 'Lead', 'Bass']);
  expect(removeEffect(project, delay.id)).toContain('Used by Master, Lead, Bass');
  removeDevice(project, -1, onMaster.device.id); removeDevice(project, 0, onLead.device.id); removeDevice(project, 1, onBass.device.id);
  expect(removeEffect(project, delay.id)).toBeNull(); expect(project.definitions.some(d => d.id === delay.id)).toBe(false);
  expect(addDevice(project, 0, 'fx:missing')).toBeNull();
  const copy = duplicateEffect(project, again);
  expect(copy.name).toBe('Echo 2 copy'); expect(copy.source).toContain('fx echo_2_copy('); expect(copy.source).not.toContain('fx echo_2(');
  expect(project.definitions.indexOf(copy)).toBe(project.definitions.indexOf(again) + 1);
});

it('reads its presets from the library that comes with niminal, each as a definition that stands alone', async () => {
  const { instrumentPresets, effectPresets, parseLibrary } = await import('./library');
  expect(instrumentPresets.map(p => p.id).slice(0, 8)).toEqual(['kick', 'snare', 'hat', 'bass', 'pluck', 'pad', 'lead', 'bell']);
  expect(instrumentPresets.map(p => p.id)).toEqual(expect.arrayContaining(['kick_808', 'snare_tri', 'clap_room', 'hat_open', 'wobble_bass', 'rim', 'cowbell']));
  expect(instrumentPresets.length).toBe(29);
  expect(Object.fromEntries(instrumentPresets.map(p => [p.id, p.folder]))).toMatchObject({ kick: 'drums/kicks', kick_808: 'drums/kicks', snare_tri: 'drums/snares', hat_open: 'drums/hats', rim: 'drums/percussion', bass: 'synths/bass', wobble_bass: 'synths/bass', zap: 'sounds/fx' });
  expect(effectPresets.every(p => p.folder.length > 0)).toBe(true); expect(effectPresets.find(p => p.id === 'echo')!.folder).toBe('time/delay');
  expect(parseLibrary('// folder: a/b\n\n/// One.\nfx one() {\n  out = in\n}\n\n/// Two.\nfx two() {\n  out = in\n}\n\n// folder: c\n\n/// Three.\nfx three() {\n  out = in\n}\n', 'fx').map(p => [p.id, p.folder])).toEqual([['one', 'a/b'], ['two', 'a/b'], ['three', 'c']]);
  expect(effectPresets.map(p => p.id)).toContain('compressor'); expect(effectPresets.length).toBeGreaterThanOrEqual(15); expect(effectPresets.find(p => p.id === 'glitch')!.folder).toBe('time/glitch');
  for (const preset of [...instrumentPresets, ...effectPresets]) {
    expect(preset.description.length).toBeGreaterThan(10); expect(preset.source).toMatch(new RegExp(`^(use .*\\n)*(///.*\\n)+(instr|fx) ${preset.id}\\(`)); expect(preset.source.trimEnd().endsWith('}')).toBe(true);
  }
  expect(instrumentPresets.find(p => p.id === 'kick')!.name).toBe('Kick'); expect(effectPresets.find(p => p.id === 'band_cut')!.name).toBe('Band cut');
  expect(parseLibrary('// header\n\n/// One.\nfx one(a: 0..1 = 0) {\n  out = in\n}\n\n/// Two.\nfx two() {\n  out = in\n}\n', 'fx').map(p => p.id)).toEqual(['one', 'two']);
  expect(parseLibrary('/// Not mine.\ninstr x(freq: hz) { osc(sine, freq) }\n', 'fx')).toEqual([]);
});

it('copies an instrument preset into a project under its own name in the code', async () => {
  const { addInstrumentFromPreset } = await import('./instruments'), { instrumentPresets } = await import('./library');
  const project = newProject(), kick = instrumentPresets.find(p => p.id === 'kick')!;
  const first = addInstrumentFromPreset(project, kick), second = addInstrumentFromPreset(project, kick);
  expect([first.name, second.name]).toEqual(['Kick', 'Kick 2']);
  expect(first.source).toContain('instr kick('); expect(second.source).toContain('instr kick_2('); expect(first.source).toContain('punch: 0..1 = 0.5');
  expect(project.definitions.filter(d => d.kind === 'instr').map(d => d.id)).toContain(second.id);
});

it('copies a drum preset together with the voice it is built on, under names that never collide', async () => {
  const { addInstrumentFromPreset, duplicateInstrument } = await import('./instruments'), { instrumentPresets } = await import('./library');
  const project = newProject(), preset = instrumentPresets.find(p => p.id === 'kick_808')!, before = project.definitions.length;
  const first = addInstrumentFromPreset(project, preset), second = addInstrumentFromPreset(project, preset), copy = duplicateInstrument(project, first);
  for (const added of [first, second, copy]) {
    const code = /^instr (\w+)\(/m.exec(added.source)![1];
    expect(added.source).toContain(`fn ${code}_voice(`); expect(added.source).toContain(`${code}_voice(\n    freq:`);
    expect(added.source).not.toMatch(/fn drum\b|\bdrum\(/);
  }
  const names = project.definitions.flatMap(d => [...d.source.matchAll(/^(?:instr|fn) (\w+)/gm)].map(m => m[1]));
  expect(new Set(names).size).toBe(names.length); expect(project.definitions.length).toBe(before + 3); expect(names.filter(n => n.endsWith('_voice'))).toHaveLength(3);
  expect(copy.source).toContain('instr kick_808_copy('); expect(copy.source).not.toContain('instr kick_808(');
});

it('adds, edits and removes an LFO on an instrument parameter by rewriting its code', () => {
  const project = newProject(), parent = document.createElement('div'); document.body.append(parent);
  const tone = project.definitions[0], changed = vi.fn();
  const parameters = { tone: [{ name: 'amp', min: 0, max: 1, default: 0.15, unit: null }, { name: 'level', min: -60, max: 6, default: 0, unit: 'db' }] };
  renderInstrument(parent, tone, parameters, vi.fn(), vi.fn(), 0, changed);
  expect(parent.querySelector<HTMLButtonElement>('[aria-label="level is a gain in decibels, which an LFO can\'t move"]')!.disabled).toBe(true);
  parent.querySelector<HTMLButtonElement>('[aria-label="Toggle amp LFO"]')!.click();
  const written = changed.mock.calls[0][0] as string;
  expect(written).toContain('// mod amp: sine 2hz 50%'); expect(written).toContain('amp = (amp + lfo(2hz, shape: sine) * 0.25).clamp(0..1)');
  tone.source = written; renderInstrument(parent, tone, parameters, vi.fn(), vi.fn(), 0, changed);
  expect(parent.querySelector('.lfo-controls')).not.toBeNull();
  const depth = parent.querySelector<HTMLInputElement>('[aria-label="amp LFO Depth %"]')!; depth.value = '100'; depth.dispatchEvent(new Event('change'));
  expect(changed.mock.calls[1][0]).toContain('// mod amp: sine 2hz 100%');
  parent.querySelector<HTMLButtonElement>('[aria-label="Toggle amp LFO"]')!.click();
  expect(changed.mock.calls[2][0]).not.toContain('mod amp');
});

it('adds macros, links effect parameters to them, and tidies up when a device or the last macro goes', () => {
  const project = newProject(); const a = addDevice(project, 0, 'echo')!, b = addDevice(project, 0, 'echo')!;
  const knob = addMacro(project, 0)!; addMacro(project, 0);
  expect(knob).toEqual({ name: 'Macro 1', value: 0, links: [] });
  linkParameter(project, 0, a.device.id, 'mix', 0); linkParameter(project, 0, b.device.id, 'mix', 0);
  linkParameter(project, 0, a.device.id, 'mix', 1); // a parameter has one driver
  expect(project.tracks[0].macros!.map(m => m.links.map(l => `${l.device}.${l.parameter}`))).toEqual([[`${b.device.id}.mix`], [`${a.device.id}.mix`]]);
  project.patterns[0].fx[0][0].target = { kind: 'macro', index: 1 };
  expect(readProject(JSON.stringify(project))).toEqual(project);
  project.patterns[0].fx[0].push({ target: { kind: 'macro', index: 1 }, envelope: { enabled: true, mode: 'hold', points: [{ row: 0, value: 0.5 }] } }, { target: { kind: 'volume' } });
  expect(removeMacro(project, 0)).toBe(true);
  expect(project.tracks[0].macros).toHaveLength(1);
  expect(project.patterns[0].fx[0].map(lane => lane.target)).toEqual([{ kind: 'volume' }]); // both macro lanes went, with their values
  removeDevice(project, 0, b.device.id);
  expect(project.tracks[0].macros![0].links).toEqual([]);
  for (let i = 0; i < 10; i++) addMacro(project, 0);
  expect(project.tracks[0].macros).toHaveLength(8);
  project.tracks[0].macros![0].links = [{ device: a.device.id, parameter: 'mix', min: 0, max: 1 }, { device: a.device.id, parameter: 'mix', min: 0, max: 1 }];
  expect(() => readProject(JSON.stringify(project))).toThrow();
});

it('shows macros in the mixer: a knob moves its value, a linked parameter hands over to it', () => {
  const project = newProject(); const a = addDevice(project, 0, 'echo')!;
  addMacro(project, 0);
  const edited = vi.fn(), write = vi.fn(), parent = document.body.appendChild(document.createElement('div'));
  const draw = () => renderMixer(parent, project, 0, edited, { [a.definition.id]: [{ name: 'mix', min: 0, max: 1, default: 0.5, unit: null }] }, write, actions({ selected: a.device.id }));
  draw();
  const knob = parent.querySelector<HTMLInputElement>('[aria-label="Macro 1 name"]')!; knob.value = 'Open'; knob.dispatchEvent(new Event('change'));
  expect(project.tracks[0].macros![0].name).toBe('Open');
  const slider = parent.querySelector<HTMLInputElement>('[aria-label="Lead macro 1"]')!; slider.value = '0.25'; slider.dispatchEvent(new Event('input'));
  expect(project.tracks[0].macros![0].value).toBe(0.25);
  const pick = parent.querySelector<HTMLSelectElement>('[aria-label="mix macro"]')!; pick.value = '0'; pick.dispatchEvent(new Event('change'));
  expect(project.tracks[0].macros![0].links).toEqual([{ device: a.device.id, parameter: 'mix', min: 0, max: 1 }]);
  expect(parent.querySelector<HTMLInputElement>('[aria-label="Lead FX mix"]')!.disabled).toBe(true);
  const low = parent.querySelector<HTMLInputElement>('[aria-label="mix macro At 0%"]')!; low.value = '25'; low.dispatchEvent(new Event('change'));
  expect(project.tracks[0].macros![0].links[0].min).toBe(0.25);
});

it('keeps one empty lane when the last macro takes the strip\'s only lane', () => {
  const project = newProject(); addMacro(project, 0);
  project.patterns[0].fx[0][0] = { target: { kind: 'macro', index: 0 }, envelope: { enabled: true, mode: 'hold', points: [{ row: 0, value: 1 }] } };
  expect(removeMacro(project, 0)).toBe(true);
  expect(project.patterns[0].fx[0]).toEqual([{ target: { kind: 'volume' } }]);
  expect(removeMacro(project, 0)).toBe(false);
});

it('keeps a strip\'s effect commands on the effects they were written for as its chain changes', () => {
  const project = newProject();
  const a = addDevice(project, 0, 'echo')!, b = addDevice(project, 0, 'echo')!, c = addDevice(project, 0, 'echo')!;
  const cells = project.patterns[0].notes[0][0].fx;
  Object.assign(cells[0], { 0: '13FF', 1: '2100', 2: '31AA', 3: 'L80', 4: '4100' }); // effect 4 does not exist yet; L80 is not an effect command
  expect(commandsFor(project, 0, b.device.id)).toBe(1);
  // moving the second effect up takes its command to place 1, and the first effect's to place 2
  expect(moveDevice(project, 0, b.device.id, -1)).toBe(true);
  expect(project.tracks[0].fx.map(d => d.id)).toEqual([b.device.id, a.device.id, c.device.id]);
  expect(cells[0].slice(0, 5)).toEqual(['23FF', '1100', '31AA', 'L80', '4100']);
  // removing the first effect clears its commands and moves the later ones up a place
  expect(removeDevice(project, 0, b.device.id)).toBeTruthy();
  expect(cells[0].slice(0, 5)).toEqual(['13FF', null, '21AA', 'L80', '4100']);
  // removing the last effect clears only its own
  expect(commandsFor(project, 0, c.device.id)).toBe(1);
  removeDevice(project, 0, c.device.id);
  expect(cells[0].slice(0, 5)).toEqual(['13FF', null, null, 'L80', '4100']);
  expect(commandsFor(project, 0, 'missing')).toBe(0);
});
