import type { Definition, Device, Project } from './project';
import { maxDevices } from './project';
import delay from './presets/delay.nml?raw';
import chorus from './presets/chorus.nml?raw';
import lowpass from './presets/lowpass.nml?raw';
import highpass from './presets/highpass.nml?raw';
import bandpass from './presets/bandpass.nml?raw';
import notch from './presets/notch.nml?raw';
import tremolo from './presets/tremolo.nml?raw';

export const builtInEffects = [
  { id: 'delay', name: 'Stereo delay', source: delay },
  { id: 'chorus', name: 'Chorus', source: chorus },
  { id: 'lowpass', name: 'Low pass', source: lowpass },
  { id: 'highpass', name: 'High pass', source: highpass },
  { id: 'bandpass', name: 'Band pass', source: bandpass },
  { id: 'notch', name: 'Notch', source: notch },
  { id: 'tremolo', name: 'Tremolo', source: tremolo },
];

const blank = { name: 'Blank', source: 'fx blank() { out = in }\n' };

/** Appends a new effect, with its own copy of the preset code, to the end of a track's chain. */
export function addDevice(project: Project, track: number, presetId: string): { device: Device; definition: Definition } | null {
  const preset = presetId === 'blank' ? blank : builtInEffects.find(p => p.id === presetId);
  const chain = project.tracks[track].fx;
  if (!preset || chain.length >= maxDevices) return null;
  const id = `fx_${crypto.randomUUID().replaceAll('-', '_')}`;
  const taken = new Set(chain.map(d => project.definitions.find(def => def.id === d.definition)?.name));
  let name = preset.name;
  for (let n = 2; taken.has(name); n++) name = `${preset.name} ${n}`;
  const definition: Definition = { id, name, kind: 'fx', source: preset.source.replace(/^fx \w+/m, `fx ${id}`) };
  const device: Device = { id: `d_${crypto.randomUUID().replaceAll('-', '_').slice(0, 12)}`, definition: id, bypass: false, values: {}, modulations: {} };
  project.definitions.push(definition); chain.push(device);
  return { device, definition };
}

/** Removes an effect and its code. Lanes that pointed at it are cleared, or kept as "missing" when they hold values. */
export function removeDevice(project: Project, track: number, deviceId: string): Definition | null {
  const chain = project.tracks[track].fx, index = chain.findIndex(d => d.id === deviceId);
  if (index < 0) return null;
  const [device] = chain.splice(index, 1);
  const definition = project.definitions.find(d => d.id === device.definition) ?? null;
  if (definition && !project.tracks.some(t => t.fx.some(d => d.definition === definition.id))) project.definitions.splice(project.definitions.indexOf(definition), 1);
  for (const pattern of project.patterns) for (const lane of pattern.fx[track]) {
    if (lane.target?.kind === 'effect' && lane.target.device === deviceId && !lane.values.some(v => v !== null) && !lane.envelope?.points.length) lane.target = null;
  }
  return definition;
}

export function moveDevice(project: Project, track: number, deviceId: string, delta: number): boolean {
  const chain = project.tracks[track].fx, from = chain.findIndex(d => d.id === deviceId), to = from + delta;
  if (from < 0 || to < 0 || to >= chain.length) return false;
  [chain[from], chain[to]] = [chain[to], chain[from]];
  return true;
}
