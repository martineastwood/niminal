import { expect, it } from 'vitest';
import { addInstrument, addKit, duplicateInstrument, instrumentUsers, removeInstrument, samplerSource, sliceInstrument } from './instruments';
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
  expect(slices.source).toContain('instr kick_slices(freq: hz, sample: num = 0, begin: 0..1 = 0, rate: -4..4 = 1, stretch: 0.25..4 = 1, stretch_mode: 0..2 = 0, from_bpm: 0..400 = 0, gate: 0..1 = 1, release: 0ms..2sec = 10ms)');
  expect(slices.source).toContain('kick_slices_slices(sample: sample, begin: begin, rate: rate, stretch: stretch, stretch_mode: stretch_mode) * (1 - gate + gate * env[1 | release 0])');
  expect(sliceInstrument(project, project.definitions[0], 8)).toBeNull();
  // at chosen places, and only within what the keyboard has keys for
  const cut = sliceInstrument(project, kick, [0, 0.123456789, 0.5])!;
  expect(cut.source).toContain('kick_slices_2_slices = kick_slices_2_sample.slices([0, 0.12346, 0.5])'); expect(cut.source).toContain('in 3 pieces');
  expect(sliceInstrument(project, kick, 1)).toBeNull(); expect(sliceInstrument(project, kick, 49)).toBeNull(); expect(sliceInstrument(project, kick, 48)).not.toBeNull();
});

it('renames the pieces of a sliced sampler along with it when it is copied', () => {
  const project = newProject(), slices = sliceInstrument(project, addInstrument(project, 'samples/amen.wav'), 8)!;
  const copy = duplicateInstrument(project, slices);
  expect(copy.source).toContain('amen_slices_copy_slices = amen_slices_copy_sample.slices(8)');
  expect(copy.source).not.toMatch(/\bamen_slices_slices\b/);
});

it('lets a long sample follow its notes and a short one play out, by default', () => {
  const project = newProject();
  expect(addInstrument(project, 'samples/kick.wav', 0.3).source).toContain('gate: 0..1 = 0');
  expect(addInstrument(project, 'samples/pad.wav', 8).source).toContain('gate: 0..1 = 1');
  expect(addInstrument(project, 'samples/unknown.wav').source).toContain('gate: 0..1 = 0');
  expect(samplerSource('hit', 'samples/hit.wav')).toContain('hit_sample(freq: freq, begin: begin, rate: rate, stretch: stretch, stretch_mode: stretch_mode) * (1 - gate + gate * env[1 | release 0])');
});

it('makes a kit instrument that lists which key plays which sample', () => {
  const project = newProject(), kit = addKit(project, 'samples/drums', ['hat', 'kick', 'snare'])!;
  expect(kit.name).toBe('drums');
  expect(kit.source).toContain('/// C-4 hat · C#4 kick · D-4 snare'); expect(kit.source).toContain('drums_kit = kit("samples/drums")');
  expect(kit.source).toContain('instr drums(freq: hz, sample: num = 0, begin: 0..1 = 0, rate: -4..4 = 1, stretch: 0.25..4 = 1, stretch_mode: 0..2 = 0, from_bpm: 0..400 = 0, gate: 0..1 = 0');
  expect(kit.source).toContain('drums_kit(sample: sample, begin: begin, rate: rate, stretch: stretch, stretch_mode: stretch_mode)');
  expect(addKit(project, 'samples/none', [])).toBeNull(); expect(addKit(project, 'samples/many', Array.from({ length: 49 }, (_, i) => `s${i}`))).toBeNull();
  const copy = duplicateInstrument(project, kit);
  expect(copy.source).toContain('instr drums_copy('); expect(copy.source).toContain('drums_copy_kit = kit("samples/drums")');
});
