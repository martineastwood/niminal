import type { Definition, Project } from './project';

const reserved = new Set(['in', 'out', 'fn', 'def', 'instr', 'fx', 'if', 'else', 'for', 'let', 'state', 'use', 'clip', 'scene', 'bus', 'tempo', 'config', 'note']);

/** A name as it is written in code: lower case words joined by `_`, starting with a letter. */
export function slug(name: string) {
  const words = name.toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '');
  if (!/^[a-z]/.test(words)) return words ? `sound_${words}` : 'sound';
  return reserved.has(words) ? `my_${words}` : words;
}

/** What a definition's code calls it: the name after `instr` or `fx`. */
export const codeNameOf = (definition: Definition) => /^(?:instr|fx) (\w+)/m.exec(definition.source)?.[1];

/** `wanted`, or `wanted_2`, `wanted_3`... when another definition's code already uses it. */
export function freeCodeName(project: Project, wanted: string, except?: Definition) {
  const taken = new Set(project.definitions.filter(d => d !== except).map(codeNameOf));
  let name = wanted;
  for (let n = 2; taken.has(name); n++) name = `${wanted}_${n}`;
  return name;
}

/** The first of `base`, `base 2`, `base 3`... that no other definition of this kind is called. */
export function uniqueName(project: Project, kind: Definition['kind'], base: string) {
  const taken = new Set(project.definitions.filter(d => d.kind === kind).map(d => d.name));
  let name = base;
  for (let n = 2; taken.has(name); n++) name = `${base} ${n}`;
  return name;
}

/** `source` with the code name `from` changed to `to`, along with the helpers named after it (`from_voice`, `from_sample`, `from_slices`, `from_kit`). Strings and comments are left alone. */
export const renameInSource = (source: string, from: string, to: string) =>
  source.replace(new RegExp(`("(?:[^"\\\\]|\\\\.)*"|//[^\\n]*)|\\b${from}(_voice|_sample|_slices|_kit)?\\b`, 'g'), (match, literal, helper) => literal ? match : `${to}${helper ?? ''}`);

/** Gives a definition a new name, and its code the matching name (made unique among the definitions). */
export function renameDefinition(project: Project, definition: Definition, name: string) {
  name = name.trim();
  if (!name) return;
  definition.name = name;
  const old = codeNameOf(definition), next = freeCodeName(project, slug(name), definition);
  if (old && old !== next) definition.source = renameInSource(definition.source, old, next);
}
