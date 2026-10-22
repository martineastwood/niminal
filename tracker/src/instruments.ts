import type { Definition, Project } from './project';

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

/** A copy with its own instrument name, and its own names for any samples it declares. */
export function duplicateInstrument(project: Project, source: Definition): Definition {
  const id = `instr_${identifier()}`, name = /^instr (\w+)/m.exec(source.source)?.[1] ?? source.id;
  let text = source.source.replace(new RegExp(`\\b${name}\\b`, 'g'), id);
  [...text.matchAll(/^(\w+)\s*=\s*sample\(/gm)].forEach((match, i) => { text = text.replace(new RegExp(`\\b${match[1]}\\b`, 'g'), `${id.replace(/^instr_/, 'smp_')}_${i}`); });
  const definition: Definition = { id, kind: 'instr', name: uniqueName(project, `${source.name} copy`), source: text };
  project.definitions.splice(project.definitions.indexOf(source) + 1, 0, definition);
  return definition;
}

export function instrumentUsers(project: Project, id: string) { return project.tracks.filter(t => t.instrument === id).map(t => t.name); }

/** Removes an instrument nobody plays. Returns why not when it cannot. */
export function removeInstrument(project: Project, id: string): string | null {
  const users = instrumentUsers(project, id);
  if (users.length) return `Used by ${users.join(', ')}: choose another instrument there first`;
  if (project.definitions.filter(d => d.kind === 'instr').length <= 1) return 'Keep at least one instrument';
  project.definitions.splice(project.definitions.findIndex(d => d.id === id), 1);
  return null;
}
