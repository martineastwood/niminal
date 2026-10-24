import { expect, it } from 'vitest';
import { addInstrument, duplicateInstrument, instrumentUsers, removeInstrument, samplerSource, sliceInstrument } from './instruments';
import { newProject } from './project';

it('adds synths and samplers with code named after them, so copies never collide', () => {
  const project = newProject();
  const synth = addInstrument(project), second = addInstrument(project), kick = addInstrument(project, 'samples/kick.wav');
  expect([synth.name, second.name, kick.name]).toEqual(['Synth', 'Synth 2', 'kick']);
  expect(synth.source).toContain('instr synth(freq: hz)'); expect(second.source).toContain('instr synth_2(freq: hz)');
  expect(kick.source).toBe(samplerSource('kick', 'samples/kick.wav'));
  expect(kick.source).toContain('sample("samples/kick.wav").root(c4)'); expect(kick.source).toContain('kick_sample');
});

it('duplicates an instrument with fresh names inside its code, including sample bindings', () => {
  const project = newProject(), kick = addInstrument(project, 'samples/kick.wav');
  const copy = duplicateInstrument(project, kick);
  expect(copy.id).not.toBe(kick.id); expect(copy.name).toBe('kick copy');
  expect(copy.source).toContain('instr kick_copy('); expect(copy.source).toContain('kick_copy_sample'); expect(copy.source).not.toContain('kick_sample'); expect(copy.source).not.toContain('instr kick(');
  expect(copy.source).toContain('sample("samples/kick.wav")');
  expect(project.definitions.indexOf(copy)).toBe(project.definitions.indexOf(kick) + 1);
  const tone = duplicateInstrument(project, project.definitions[0]);
  expect(tone.source).toMatch(/^instr warm_saw_copy\(/m);
});

it('keeps sample bindings of older projects from colliding when it duplicates them', () => {
  const project = newProject();
  const old = { id: 'instr_x', kind: 'instr' as const, name: 'Old', source: 'smp_x = sample("a.wav")\ninstr instr_x(freq: hz) {\n  smp_x(freq: freq)\n}\n' };
  project.definitions.push(old);
  const copy = duplicateInstrument(project, old);
  expect(copy.source).toContain('instr old_copy('); expect(copy.source).toContain('old_copy_sample_0 = sample'); expect(copy.source).not.toContain('smp_x');
});

it('counts an instrument chosen in an instrument column as played, so it cannot be deleted', () => {
  const project = newProject(), extra = addInstrument(project);
  project.patterns[0].notes[2][0].inst[3] = extra.id;
  expect(instrumentUsers(project, extra.id)).toEqual(['Drums']);
  expect(removeInstrument(project, extra.id)).toContain('Drums');
});

it('only removes an instrument that no note uses, and keeps one instrument', () => {
  const project = newProject(), extra = addInstrument(project), bass = project.patterns[0].notes[1][0];
  expect(instrumentUsers(project, 'tone')).toEqual(['Lead']);
  expect(removeInstrument(project, 'tone')).toContain('Used by Lead');
  bass.inst[0] = extra.id;
  expect(removeInstrument(project, extra.id)).toContain('Bass');
  bass.inst[0] = null;
  expect(removeInstrument(project, extra.id)).toBeNull();
  expect(project.definitions.some(d => d.id === extra.id)).toBe(false);
  project.patterns[0].notes[0][0].inst.fill(null);
  expect(removeInstrument(project, 'tone')).toBe('Keep at least one instrument');
});

it('makes a sliced copy of a sampler, and nothing from an instrument with no sample', () => {
  const project = newProject(), kick = addInstrument(project, 'samples/kick.wav');
  const slices = sliceInstrument(project, kick, 16)!;
  expect(slices.name).toBe('kick slices'); expect(slices.id).not.toBe(kick.id);
  expect(slices.source).toContain('sample("samples/kick.wav")'); expect(slices.source).toContain('kick_slices_slices = kick_slices_sample.slices(16)');
  expect(slices.source).toContain('instr kick_slices(freq: hz, sample: num = 0, begin: 0..1 = 0, rate: -4..4 = 1)');
  expect(sliceInstrument(project, project.definitions[0], 8)).toBeNull();
});
