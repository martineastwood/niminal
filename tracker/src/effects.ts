import type { Definition, Device, Macro, Project, Track } from './project';
import { columnLanes, emptyFxLane, maxDevices, maxMacros } from './project';
import send from './presets/send.nml?raw';
import { effectPresets } from './library';
import { codeNameOf, freeCodeName, renameInSource, slug, uniqueName } from './naming';

/** The effects that come with niminal (`std/fx`), each copied into a project as it is added. */
export const builtInEffects = effectPresets;

/** The preset that makes a send device; offered on tracks only. */
export const sendPreset = { id: 'send', name: 'Send', source: send };

const blank = { name: 'Blank', source: 'fx blank() { out = in }\n' };

/** A strip is a track number (0 and up), `master`, or a return: `returnStrip(0)` and below. */
export const master = -1;
export const returnStrip = (index: number) => -2 - index;
export const returnIndex = (strip: number) => -2 - strip;
/** The strip number of a column of the pattern grid (tracks, then the returns, then the master), and the column of a strip. */
export const columnToStrip = (project: Project, column: number) => column < project.tracks.length ? column : column < project.tracks.length + project.returns.length ? returnStrip(column - project.tracks.length) : master;
export const stripToColumn = (project: Project, strip: number) => strip >= 0 ? strip : project.tracks.length + (strip === master ? project.returns.length : returnIndex(strip));
export const stripOf = (project: Project, strip: number): Track => strip >= 0 ? project.tracks[strip] : strip === master ? project.master : project.returns[returnIndex(strip)];

/** A new effect in the project's library, from a preset or blank. Its code is named after it, made unique so copies never collide. */
export function newEffect(project: Project, presetId = 'blank'): Definition | null {
  const preset = presetId === 'blank' ? blank : presetId === sendPreset.id ? sendPreset : builtInEffects.find(p => p.id === presetId);
  if (!preset) return null;
  const name = uniqueName(project, 'fx', preset.name), code = freeCodeName(project, slug(name));
  const definition: Definition = { id: `fx_${crypto.randomUUID().replaceAll('-', '_')}`, name, kind: 'fx', source: preset.source.replace(/^fx \w+/m, `fx ${code}`) };
  project.definitions.push(definition);
  return definition;
}

/** A copy with its own name in the code, placed after the original. */
export function duplicateEffect(project: Project, source: Definition): Definition {
  const name = uniqueName(project, 'fx', `${source.name} copy`), code = freeCodeName(project, slug(name)), old = codeNameOf(source);
  const definition: Definition = { id: `fx_${crypto.randomUUID().replaceAll('-', '_')}`, kind: 'fx', name, source: old ? renameInSource(source.source, old, code) : source.source };
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

/** Adds a macro knob to a strip, or returns null when it has all it can. */
export function addMacro(project: Project, track: number): Macro | null {
  const strip = stripOf(project, track), macros = strip.macros ??= [];
  if (macros.length >= maxMacros) return null;
  const knob: Macro = { name: `Macro ${macros.length + 1}`, value: 0, links: [] };
  macros.push(knob);
  return knob;
}

/** Removes a strip's last macro (the others keep their numbers, which lanes refer to), and the lanes on it. A strip keeps at least one lane, which is left empty. Says whether lanes went. */
export function removeMacro(project: Project, track: number): boolean {
  const macros = stripOf(project, track).macros, index = (macros?.length ?? 0) - 1;
  if (!macros || index < 0) return false;
  macros.pop();
  const column = stripToColumn(project, track);
  let removed = false;
  for (const pattern of project.patterns) {
    const lanes = columnLanes(project, pattern, column);
    for (let i = lanes.length - 1; i >= 0; i--) {
      const target = lanes[i].target;
      if (target?.kind !== 'macro' || target.index !== index) continue;
      removed = true;
      if (lanes.length > 1) lanes.splice(i, 1); else lanes[i] = emptyFxLane(lanes[i].values.length);
    }
  }
  return removed;
}

/** Has macro `index` (or none, for null) drive an effect parameter, over its whole range to begin with. A parameter has one driver. */
export function linkParameter(project: Project, track: number, device: string, parameter: string, index: number | null) {
  const macros = stripOf(project, track).macros ?? [];
  for (const knob of macros) knob.links = knob.links.filter(link => link.device !== device || link.parameter !== parameter);
  if (index !== null) macros[index]?.links.push({ device, parameter, min: 0, max: 1 });
}

/** Removes a device from a chain. The effect stays in the library. Lanes that pointed at it are cleared, or kept as "missing" when they hold values. */
export function removeDevice(project: Project, track: number, deviceId: string): Device | null {
  const chain = stripOf(project, track).fx, index = chain.findIndex(d => d.id === deviceId);
  if (index < 0) return null;
  const [device] = chain.splice(index, 1);
  for (const knob of stripOf(project, track).macros ?? []) knob.links = knob.links.filter(link => link.device !== deviceId);
  const column = stripToColumn(project, track);
  for (const pattern of project.patterns) for (const lane of columnLanes(project, pattern, column)) {
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
