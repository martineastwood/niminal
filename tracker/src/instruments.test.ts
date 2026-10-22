import { expect, it } from 'vitest';
import { addInstrument, duplicateInstrument, instrumentUsers, removeInstrument, samplerSource } from './instruments';
import { newProject } from './project';

it('adds synths and samplers with names derived from their ids, so copies never collide', () => {
  const project = newProject();
  const synth = addInstrument(project), second = addInstrument(project), kick = addInstrument(project, 'samples/kick.wav');
  expect([synth.name, second.name, kick.name]).toEqual(['Synth', 'Synth 2', 'kick']);
  expect(synth.source).toContain(`instr ${synth.id}(freq: hz)`);
  expect(kick.source).toBe(samplerSource(kick.id, 'samples/kick.wav'));
  expect(kick.source).toContain('sample("samples/kick.wav").root(c4)');
  expect(kick.source).toContain(kick.id.replace('instr_', 'smp_'));
});

it('duplicates an instrument with fresh names inside its code, including sample bindings', () => {
  const project = newProject(), kick = addInstrument(project, 'samples/kick.wav');
  const copy = duplicateInstrument(project, kick);
  expect(copy.id).not.toBe(kick.id); expect(copy.name).toBe('kick copy');
  expect(copy.source).toContain(`instr ${copy.id}(`); expect(copy.source).not.toContain(kick.id);
  expect(copy.source).not.toContain(kick.id.replace('instr_', 'smp_'));
  expect(copy.source).toContain('sample("samples/kick.wav")');
  expect(project.definitions.indexOf(copy)).toBe(project.definitions.indexOf(kick) + 1);
  const tone = duplicateInstrument(project, project.definitions[0]);
  expect(tone.source).toMatch(new RegExp(`^instr ${tone.id}\\(`, 'm'));
});

it('counts an instrument chosen in an instrument column as played, so it cannot be deleted', () => {
  const project = newProject(), extra = addInstrument(project);
  project.patterns[0].notes[2][0].inst[3] = extra.id;
  expect(instrumentUsers(project, extra.id)).toEqual(['Drums']);
  expect(removeInstrument(project, extra.id)).toContain('Drums');
});

it('only removes an instrument that no track plays, and keeps one instrument', () => {
  const project = newProject(), extra = addInstrument(project);
  expect(instrumentUsers(project, 'tone')).toEqual(['Lead']);
  expect(removeInstrument(project, 'tone')).toContain('Used by Lead');
  project.tracks[1].instrument = extra.id;
  expect(removeInstrument(project, extra.id)).toContain('Bass');
  project.tracks[1].instrument = null;
  expect(removeInstrument(project, extra.id)).toBeNull();
  expect(project.definitions.some(d => d.id === extra.id)).toBe(false);
  project.tracks[0].instrument = null;
  expect(removeInstrument(project, 'tone')).toBe('Keep at least one instrument');
});
