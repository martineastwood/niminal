import type { Definition, Project } from './project';
import type { Preset } from './library';

const identifier = () => crypto.randomUUID().replaceAll('-', '_');

const synthSource = (id: string) => `/// Describe your instrument here.\ninstr ${id}(freq: hz) {\n  osc(sine, freq) * env[0.15 | 200ms 0]\n}\n`;

/** Plays one imported WAV, pitched from its root note. `file` is relative to the project folder. */
export const samplerSource = (id: string, file: string) => {
  const sample = id.replace(/^instr_/, 'smp_');
  return `/// Plays ${file}. Change .root(...) to retune, or add .loop(forward) to sustain it.\n${sample} = sample("${file}").root(c4)\ninstr ${id}(freq: hz) {\n  ${sample}(freq: freq)\n}\n`;
};

function uniqueName(project: Project, base: string) {
  const taken = new Set(project.definitions.filter(d => d.kind === 'instr').map(d => d.name));
  let name = base;
  for (let n = 2; taken.has(name); n++) name = `${base} ${n}`;
  return name;
}

/** A new synth, or a sampler for an imported file. Names inside the code come from the id, so they never collide. */
export function addInstrument(project: Project, sample?: string): Definition {
  const id = `instr_${identifier()}`;
  const stem = sample?.split('/').pop()?.replace(/\.wav$/i, '');
  const definition: Definition = { id, kind: 'instr', name: uniqueName(project, stem ?? 'Synth'), source: sample ? samplerSource(id, sample) : synthSource(id) };
  project.definitions.push(definition);
  return definition;
}

/** A copy of one of the instruments that come with niminal, named after its id in the code so it never collides. */
export function addInstrumentFromPreset(project: Project, preset: Preset): Definition {
  const id = `instr_${identifier()}`;
  const source = preset.source.replace(/^instr \w+/m, `instr ${id}`);
  // a drum preset carries its own copy of the voice it calls, named from the id like the instrument
  const voice = preset.voice?.replace(/^fn drum\b/m, `fn ${id}_voice`).replace(/^\/\/\/.*\n/gm, '');
  const definition: Definition = { id, kind: 'instr', name: uniqueName(project, preset.name), source: voice ? `${source.replace(/\bdrum\(/, `${id}_voice(`)}\n${voice}` : source };
  project.definitions.push(definition);
  return definition;
}

/** A copy with its own instrument name, and its own names for any samples it declares. */
export function duplicateInstrument(project: Project, source: Definition): Definition {
  const id = `instr_${identifier()}`, name = /^instr (\w+)/m.exec(source.source)?.[1] ?? source.id;
  let text = source.source.replace(new RegExp(`\\b${name}(_voice)?\\b`, 'g'), `${id}$1`);
  [...text.matchAll(/^(\w+)\s*=\s*sample\(/gm)].forEach((match, i) => { text = text.replace(new RegExp(`\\b${match[1]}\\b`, 'g'), `${id.replace(/^instr_/, 'smp_')}_${i}`); });
  const definition: Definition = { id, kind: 'instr', name: uniqueName(project, `${source.name} copy`), source: text };
  project.definitions.splice(project.definitions.indexOf(source) + 1, 0, definition);
  return definition;
}

/** Who plays an instrument: the tracks it is assigned to, and tracks whose instrument columns pick it. */
export function instrumentUsers(project: Project, id: string) {
  const names = new Set(project.tracks.filter(t => t.instrument === id).map(t => t.name));
  project.tracks.forEach((track, index) => { if (project.patterns.some(p => p.notes[index].some(column => column.inst.includes(id)))) names.add(track.name); });
  return [...names];
}

/** Removes an instrument nobody plays. Returns why not when it cannot. */
export function removeInstrument(project: Project, id: string): string | null {
  const users = instrumentUsers(project, id);
  if (users.length) return `Used by ${users.join(', ')}: choose another instrument there first`;
  if (project.definitions.filter(d => d.kind === 'instr').length <= 1) return 'Keep at least one instrument';
  project.definitions.splice(project.definitions.findIndex(d => d.id === id), 1);
  return null;
}
