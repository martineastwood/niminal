import type { Definition, Device, Project, Track } from './project';
import { maxDevices } from './project';
import send from './presets/send.nml?raw';
import { effectPresets } from './library';

/** The effects that come with niminal (`std/fx`), each copied into a project as it is added. */
export const builtInEffects = effectPresets;

/** The preset that makes a send device; offered on tracks only. */
export const sendPreset = { id: 'send', name: 'Send', source: send };

const blank = { name: 'Blank', source: 'fx blank() { out = in }\n' };

/** A strip is a track number (0 and up), `master`, or a return: `returnStrip(0)` and below. */
export const master = -1;
export const returnStrip = (index: number) => -2 - index;
export const returnIndex = (strip: number) => -2 - strip;
export const stripOf = (project: Project, strip: number): Track => strip >= 0 ? project.tracks[strip] : strip === master ? project.master : project.returns[returnIndex(strip)];

const effectName = (project: Project, base: string) => {
  const taken = new Set(project.definitions.filter(d => d.kind === 'fx').map(d => d.name));
  let name = base;
  for (let n = 2; taken.has(name); n++) name = `${base} ${n}`;
  return name;
};

/** A new effect in the project's library, from a preset or blank. Its code names itself after its id, so copies never collide. */
export function newEffect(project: Project, presetId = 'blank'): Definition | null {
  const preset = presetId === 'blank' ? blank : presetId === sendPreset.id ? sendPreset : builtInEffects.find(p => p.id === presetId);
  if (!preset) return null;
  const id = `fx_${crypto.randomUUID().replaceAll('-', '_')}`;
  const definition: Definition = { id, name: effectName(project, preset.name), kind: 'fx', source: preset.source.replace(/^fx \w+/m, `fx ${id}`) };
  project.definitions.push(definition);
  return definition;
}

/** A copy with its own name in the code, placed after the original. */
export function duplicateEffect(project: Project, source: Definition): Definition {
  const id = `fx_${crypto.randomUUID().replaceAll('-', '_')}`, name = /^fx (\w+)/m.exec(source.source)?.[1] ?? source.id;
  const definition: Definition = { id, kind: 'fx', name: effectName(project, `${source.name} copy`), source: source.source.replace(new RegExp(`\\b${name}\\b`, 'g'), id) };
  project.definitions.splice(project.definitions.indexOf(source) + 1, 0, definition);
  return definition;
}

/** Where an effect is used: the names of the tracks (or Master) that have a device running it. */
export function effectUsers(project: Project, id: string): string[] {
  return [project.master, ...project.returns, ...project.tracks].filter(t => t.fx.some(d => d.definition === id)).map(t => t === project.master ? 'Master' : t.name);
}

/** Removes an effect from the library unless a device runs it. Returns why not. */
export function removeEffect(project: Project, id: string): string | null {
  const users = effectUsers(project, id);
  if (users.length) return `Used by ${users.join(', ')}: remove it there first`;
  project.definitions.splice(project.definitions.findIndex(d => d.id === id), 1);
  return null;
}

/**
 * Appends a device to the end of a strip's chain. `choice` is `fx:<id>` to run an effect already in the
 * library (so edits to its code reach every device using it), or a preset id or `blank` for a new one.
 */
export function addDevice(project: Project, track: number, choice: string): { device: Device; definition: Definition } | null {
  const chain = stripOf(project, track).fx;
  if (chain.length >= maxDevices) return null;
  const definition = choice.startsWith('fx:') ? project.definitions.find(d => d.id === choice.slice(3) && d.kind === 'fx') : newEffect(project, choice);
  if (!definition) return null;
  const device: Device = { id: `d_${crypto.randomUUID().replaceAll('-', '_').slice(0, 12)}`, definition: definition.id, bypass: false, values: {}, modulations: {}, ...(choice === sendPreset.id ? { target: track < -1 ? returnIndex(track) + 1 : 0 } : {}) };
  chain.push(device);
  return { device, definition };
}

/** Removes a device from a chain. The effect stays in the library. Lanes that pointed at it are cleared, or kept as "missing" when they hold values. */
export function removeDevice(project: Project, track: number, deviceId: string): Device | null {
  const chain = stripOf(project, track).fx, index = chain.findIndex(d => d.id === deviceId);
  if (index < 0) return null;
  const [device] = chain.splice(index, 1);
  if (track >= 0) for (const pattern of project.patterns) for (const lane of pattern.fx[track]) {
    if (lane.target?.kind === 'effect' && lane.target.device === deviceId && !lane.values.some(v => v !== null) && !lane.envelope?.points.length) lane.target = null;
  }
  return device;
}

export function moveDevice(project: Project, track: number, deviceId: string, delta: number): boolean {
  const chain = stripOf(project, track).fx, from = chain.findIndex(d => d.id === deviceId), to = from + delta;
  if (from < 0 || to < 0 || to >= chain.length) return false;
  [chain[from], chain[to]] = [chain[to], chain[from]];
  return true;
}
