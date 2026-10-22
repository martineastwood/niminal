import type { Definition, Project } from './project';
import delay from './presets/delay.nml?raw';
import chorus from './presets/chorus.nml?raw';
import lowpass from './presets/lowpass.nml?raw';
import highpass from './presets/highpass.nml?raw';
import bandpass from './presets/bandpass.nml?raw';
import notch from './presets/notch.nml?raw';

export const builtInEffects = [
  { id: 'delay', name: 'Stereo delay', source: delay },
  { id: 'chorus', name: 'Chorus', source: chorus },
  { id: 'lowpass', name: 'Low pass', source: lowpass },
  { id: 'highpass', name: 'High pass', source: highpass },
  { id: 'bandpass', name: 'Band pass', source: bandpass },
  { id: 'notch', name: 'Notch', source: notch },
];

// A project-owned copy keeps edits local and avoids top-level symbol collisions.
export function copyBuiltInEffect(project: Project, presetId: string): Definition {
  const preset = builtInEffects.find(preset => preset.id === presetId);
  if (!preset) throw new Error('Unknown built-in effect');
  const id = `fx_${crypto.randomUUID().replaceAll('-', '_')}`;
  const definition: Definition = { id, name: preset.name, kind: 'fx', source: preset.source.replace(/^fx \w+/m, `fx ${id}`) };
  project.definitions.push(definition);
  return definition;
}
