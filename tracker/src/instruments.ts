import { noteAt, type Definition, type Project } from './project';
import type { Preset } from './library';
import { guessPlayback, writePlayback } from './playback';
import { codeNameOf, freeCodeName, renameInSource, slug, uniqueName } from './naming';

const identifier = () => crypto.randomUUID().replaceAll('-', '_');

const synthSource = (code: string) => `/// Describe your instrument here.\ninstr ${code}(freq: hz) {\n  osc(sine, freq) * env[0.15 | 200ms 0]\n}\n`;

/** The note commands O (start part way in) and V (backwards) set `begin` and `rate`, so a sampler declares them. */
const sampleParams = 'begin: 0..1 = 0, rate: -4..4 = 1, stretch: 0.25..4 = 1, stretch_mode: 0..2 = 0, from_bpm: 0..400 = 0';
/**
 * `gate` decides what a note's end does to a sampler: 0 plays the whole sample whatever the note, 1 ends it, over `release`,
 * where the note ends (an OFF, CUT or the next note in the column), so a released pad or loop can be stopped.
 */
const gateParams = (gate: number) => `gate: 0..1 = ${gate}, release: 0ms..2sec = 10ms`;
const samplerParams = (gate: number, sliced = false) =>
  ['freq: hz', ...(sliced ? ['sample: num = 0'] : []), ...sampleParams.split(', '), ...gateParams(gate).split(', ')].join(',\n  ');
const gated = (play: string) => `${play} * (1 - gate + gate * env[1 | release 0])`;
const gateNote = '/// gate: 0 plays the whole sample, 1 ends it when the note does (OFF, CUT or the next note), fading over release.';

/** Samples this long or longer (loops, pads, breaks) follow their notes; shorter ones are hits that play out. */
export const longSample = 2;

/** Plays one imported audio file, pitched from its root note. `file` is relative to the project folder; `seconds` is how long it plays, if known. */
export const samplerSource = (code: string, file: string, seconds = 0) =>
  `/// Plays ${file}. Change .root(...) to retune, or add .loop(forward) to sustain it.\n${gateNote}\n${code}_sample = sample("${file}").root(c4)\ninstr ${code}(\n  ${samplerParams(seconds >= longSample ? 1 : 0)}\n) {\n  ${gated(`${code}_sample(freq: freq, begin: begin, rate: rate, stretch: stretch, stretch_mode: stretch_mode)`)}\n}\n`;

/** The most pieces a sample can be cut into, or samples a kit can hold: one for each key from C-4 to B-7. */
export const maxSlices = 48;

/** Plays `file` cut into `pieces` equal pieces, or at the given shares of its length: each key from C-4 up plays the next piece, at its recorded pitch. */
export const slicedSource = (code: string, file: string, pieces: number | number[]) => {
  const count = typeof pieces === 'number' ? pieces : pieces.length;
  const cut = typeof pieces === 'number' ? String(pieces) : `[${pieces.map(share => Number(share.toFixed(5))).join(', ')}]`;
  return `/// Plays ${file} in ${count} pieces: C-4 is the first, C#4 the next, and so on.\n${gateNote}\n${code}_sample = sample("${file}").root(c4)\n${code}_slices = ${code}_sample.slices(${cut})\ninstr ${code}(\n  ${samplerParams(1, true)}\n) {\n  ${gated(`${code}_slices(sample: sample, begin: begin, rate: rate, stretch: stretch, stretch_mode: stretch_mode)`)}\n}\n`;
};

/** Plays the samples of a kit folder, one key each from C-4: `members` are their names in the order the engine counts them. */
export const kitSource = (code: string, folder: string, members: string[]) => {
  const keys = members.map((member, index) => `${noteAt(48 + index) ?? '?'} ${member}`);
  const lines = keys.reduce<string[]>((all, key) => { const last = all.at(-1); if (last !== undefined && last.length + key.length < 90) all[all.length - 1] = `${last} · ${key}`; else all.push(key); return all; }, []);
  return `/// Plays the samples in ${folder}, one key each:\n${lines.map(line => `/// ${line}`).join('\n')}\n${gateNote}\n${code}_kit = kit("${folder}")\ninstr ${code}(\n  ${samplerParams(0, true)}\n) {\n  ${gated(`${code}_kit(sample: sample, begin: begin, rate: rate, stretch: stretch, stretch_mode: stretch_mode)`)}\n}\n`;
};

/** A new instrument called `base` (made unique), whose code is written for the name it ends up with. */
function create(project: Project, base: string, write: (code: string) => string): Definition {
  const name = uniqueName(project, 'instr', base), code = freeCodeName(project, slug(name));
  const definition: Definition = { id: `instr_${identifier()}`, kind: 'instr', name, source: write(code) };
  project.definitions.push(definition);
  return definition;
}

/** A new synth, or a sampler for an imported file (`seconds` long, if known). */
export function addInstrument(project: Project, sample?: string, seconds = 0): Definition {
  const stem = sample?.split('/').pop()?.replace(/\.[^.]+$/, '');
  return create(project, stem ?? 'Synth', code => sample ? writePlayback(samplerSource(code, sample, seconds), guessPlayback(stem ?? '', seconds, longSample)) : synthSource(code));
}

/** A new instrument that plays the samples of an imported kit folder, one key each from C-4, or null when there are none or too many. */
export function addKit(project: Project, folder: string, members: string[]): Definition | null {
  if (!members.length || members.length > maxSlices) return null;
  return create(project, folder.split('/').pop() ?? 'Kit', code => kitSource(code, folder, members));
}

/** The file a sampler's code plays, or undefined when it plays none. */
export const sampleFile = (source: Definition) => /\bsample\("([^"]+)"\)/.exec(source.source)?.[1];

/** The code of a sampler with its sample changed to `file`, or null when `source` plays no sample. */
export function withSample(source: string, file: string): string | null {
  const old = /\bsample\("([^"]+)"\)/.exec(source)?.[1];
  return old ? source.replaceAll(old, () => file) : null;
}

/** A new instrument that plays the sample of a sampler cut into `pieces` (a count, or the shares of its length where each starts), or null when `source` plays no sample. */
export function sliceInstrument(project: Project, source: Definition, pieces: number | number[]): Definition | null {
  const file = sampleFile(source), count = typeof pieces === 'number' ? pieces : pieces.length;
  return file && count >= 2 && count <= maxSlices ? create(project, `${source.name} slices`, code => slicedSource(code, file, pieces)) : null;
}

/** The code of a preset under the name `code`; a drum preset carries its own copy of the voice it calls, named from it. */
export function presetSource(preset: Preset, code: string) {
  const source = preset.source.replace(/^instr \w+/m, `instr ${code}`);
  const voice = preset.helpers?.replace(/^fn drum\b/m, `fn ${code}_voice`).replace(/^\/\/\/.*\n/gm, '');
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
