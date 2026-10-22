import type { Definition, Project } from './project';
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

/** Each track owns its processor; presets replace source without changing its automation address. */
export function setTrackFxPreset(project: Project, track: number, presetId = 'dry'): Definition {
  const source = presetId === 'dry' ? 'fx passthrough() { out = in }\n' : builtInEffects.find(p => p.id === presetId)?.source;
  if (!source) throw new Error('Unknown track FX preset');
  const slot = project.tracks[track].fx;
  const definition = slot && project.definitions.find(d => d.id === slot.definition);
  const id = definition?.id ?? `fx_${crypto.randomUUID().replaceAll('-', '_')}`;
  const fx: Definition = definition ?? { id, name: `${project.tracks[track].name} FX`, kind: 'fx', source: '' };
  fx.source = source.replace(/^fx \w+/m, `fx ${id}`);
  if (!definition) {
    project.definitions.push(fx);
    project.tracks[track].fx = { definition: id, values: {}, modulations: {} };
  }
  return fx;
}
