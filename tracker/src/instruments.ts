import type { Definition, Project } from './project';
import type { Preset } from './library';
import { codeNameOf, freeCodeName, renameInSource, slug, uniqueName } from './naming';

const identifier = () => crypto.randomUUID().replaceAll('-', '_');

const synthSource = (code: string) => `/// Describe your instrument here.\ninstr ${code}(freq: hz) {\n  osc(sine, freq) * env[0.15 | 200ms 0]\n}\n`;

/** Plays one imported WAV, pitched from its root note. `file` is relative to the project folder. */
export const samplerSource = (code: string, file: string) =>
  `/// Plays ${file}. Change .root(...) to retune, or add .loop(forward) to sustain it.\n${code}_sample = sample("${file}").root(c4)\ninstr ${code}(freq: hz) {\n  ${code}_sample(freq: freq)\n}\n`;

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

/** Who plays an instrument: the tracks with a note column that sets it. */
export function instrumentUsers(project: Project, id: string) {
  return project.tracks.filter((_, index) => project.patterns.some(p => p.notes[index].some(column => column.inst.includes(id)))).map(t => t.name);
}

/** Removes an instrument no note uses. Returns why not when it cannot. */
export function removeInstrument(project: Project, id: string): string | null {
  const users = instrumentUsers(project, id);
  if (users.length) return `Used by ${users.join(', ')}: choose another instrument there first`;
  if (project.definitions.filter(d => d.kind === 'instr').length <= 1) return 'Keep at least one instrument';
  project.definitions.splice(project.definitions.findIndex(d => d.id === id), 1);
  return null;
}
