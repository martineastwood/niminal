import type { Definition, Project } from './project';
import type { Preset } from './library';
import { codeNameOf, freeCodeName, renameInSource, slug, uniqueName } from './naming';

const identifier = () => crypto.randomUUID().replaceAll('-', '_');

const synthSource = (code: string) => `/// Describe your instrument here.\ninstr ${code}(freq: hz) {\n  osc(sine, freq) * env[0.15 | 200ms 0]\n}\n`;

/** The note commands O (start part way in) and V (backwards) set `begin` and `rate`, so a sampler declares them. */
const samplerParams = 'begin: 0..1 = 0, rate: -4..4 = 1';

/** Plays one imported WAV, pitched from its root note. `file` is relative to the project folder. */
export const samplerSource = (code: string, file: string) =>
  `/// Plays ${file}. Change .root(...) to retune, or add .loop(forward) to sustain it.\n${code}_sample = sample("${file}").root(c4)\ninstr ${code}(freq: hz, ${samplerParams}) {\n  ${code}_sample(freq: freq, begin: begin, rate: rate)\n}\n`;

/** Plays `file` cut into `count` equal pieces: each key from C-4 up plays the next piece, at its recorded pitch. */
export const slicedSource = (code: string, file: string, count: number) =>
  `/// Plays ${file} in ${count} pieces: C-4 is the first, C#4 the next, and so on.\n${code}_sample = sample("${file}").root(c4)\n${code}_slices = ${code}_sample.slices(${count})\ninstr ${code}(freq: hz, sample: num = 0, ${samplerParams}) {\n  ${code}_slices(sample: sample, begin: begin, rate: rate)\n}\n`;

/** A new instrument called `base` (made unique), whose code is written for the name it ends up with. */
function create(project: Project, base: string, write: (code: string) => string): Definition {
  const name = uniqueName(project, 'instr', base), code = freeCodeName(project, slug(name));
  const definition: Definition = { id: `instr_${identifier()}`, kind: 'instr', name, source: write(code) };
  project.definitions.push(definition);
  return definition;
}

/** A new synth, or a sampler for an imported file. */
export function addInstrument(project: Project, sample?: string): Definition {
  const stem = sample?.split('/').pop()?.replace(/\.wav$/i, '');
  return create(project, stem ?? 'Synth', code => sample ? samplerSource(code, sample) : synthSource(code));
}

/** A new instrument that plays the sample of a sampler cut into `count` pieces, or null when `source` plays no sample. */
export function sliceInstrument(project: Project, source: Definition, count: number): Definition | null {
  const file = /\bsample\("([^"]+)"\)/.exec(source.source)?.[1];
  return file ? create(project, `${source.name} slices`, code => slicedSource(code, file, count)) : null;
}

/** The code of a preset under the name `code`; a drum preset carries its own copy of the voice it calls, named from it. */
export function presetSource(preset: Preset, code: string) {
  const source = preset.source.replace(/^instr \w+/m, `instr ${code}`);
  const voice = preset.voice?.replace(/^fn drum\b/m, `fn ${code}_voice`).replace(/^\/\/\/.*\n/gm, '');
  return voice ? `${source.replace(/\bdrum\(/, `${code}_voice(`)}\n${voice}` : source;
}

/** A copy of one of the instruments that come with niminal, under its own name so it never collides. */
export function addInstrumentFromPreset(project: Project, preset: Preset, name = preset.name): Definition {
  return create(project, name, code => presetSource(preset, code));
}

/** A copy with its own names in the code, placed after the original. */
export function duplicateInstrument(project: Project, source: Definition): Definition {
  const name = uniqueName(project, 'instr', `${source.name} copy`), code = freeCodeName(project, slug(name)), old = codeNameOf(source);
  let text = old ? renameInSource(source.source, old, code) : source.source;
  // samples bound under names of their own (older projects) get this copy's, since all the code shares one namespace
  [...text.matchAll(/^(\w+)\s*=\s*sample\(/gm)].filter(match => !match[1].startsWith(`${code}_`)).forEach((match, i) => { text = text.replace(new RegExp(`\\b${match[1]}\\b`, 'g'), `${code}_sample_${i}`); });
  const definition: Definition = { id: `instr_${identifier()}`, kind: 'instr', name, source: text };
  project.definitions.splice(project.definitions.indexOf(source) + 1, 0, definition);
  return definition;
}

/** The instruments a track's notes use: those set in any of its note columns, in any pattern. */
export function instrumentsOnTrack(project: Project, track: number): Set<string> {
  return new Set(project.patterns.flatMap(p => p.notes[track].flatMap(column => column.inst)).filter((id): id is string => !!id));
}

/** Who plays an instrument: the tracks with a note column that sets it. */
export function instrumentUsers(project: Project, id: string) {
  return project.tracks.filter((_, index) => instrumentsOnTrack(project, index).has(id)).map(t => t.name);
}

/** Removes an instrument no note uses. Returns why not when it cannot. */
export function removeInstrument(project: Project, id: string): string | null {
  const users = instrumentUsers(project, id);
  if (users.length) return `Used by ${users.join(', ')}: choose another instrument there first`;
  if (project.definitions.filter(d => d.kind === 'instr').length <= 1) return 'Keep at least one instrument';
  project.definitions.splice(project.definitions.findIndex(d => d.id === id), 1);
  return null;
}
