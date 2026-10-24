import { expect, it } from 'vitest';
import { renameDefinition, slug } from './naming';
import { addInstrument, addInstrumentFromPreset } from './instruments';
import { instrumentPresets } from './library';
import { newProject } from './project';

it('writes a name as a code identifier that never starts with a digit or reuses a keyword', () => {
  expect(slug('Warm saw')).toBe('warm_saw'); expect(slug('  Lead  #2 ')).toBe('lead_2'); expect(slug('808 kick')).toBe('sound_808_kick');
  expect(slug('')).toBe('sound'); expect(slug('Out')).toBe('my_out');
});

it('renames an instrument and its code together, keeping the code names unique', () => {
  const project = newProject(), synth = addInstrument(project), kick = addInstrumentFromPreset(project, instrumentPresets.find(p => p.id === 'kick_808')!);
  renameDefinition(project, synth, 'Sub bass');
  expect(synth.name).toBe('Sub bass'); expect(synth.source).toContain('instr sub_bass(freq: hz)');
  renameDefinition(project, kick, 'Sub bass');
  expect(kick.source).toContain('instr sub_bass_2('); expect(kick.source).toContain('fn sub_bass_2_voice('); expect(kick.source).not.toContain('kick_808');
  renameDefinition(project, kick, '   '); expect(kick.name).toBe('Sub bass');
  renameDefinition(project, synth, 'Sub bass'); expect(synth.source).toContain('instr sub_bass(');
});

it('names an instrument from a preset after the preset, or after the preset and track', () => {
  const project = newProject(), pluck = instrumentPresets.find(p => p.id === 'pluck')!;
  expect(addInstrumentFromPreset(project, pluck, 'Bass pluck').source).toContain('instr bass_pluck(');
});
