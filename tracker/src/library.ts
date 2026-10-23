import type { Definition } from './project';
import instruments from '../../crates/niminal-lang/std/instruments.nml?raw';
import fx from '../../crates/niminal-lang/std/fx.nml?raw';

/** One ready-made instrument or effect from the library that comes with niminal (`use std/instruments`, `use std/fx`). */
export interface Preset { id: string; name: string; description: string; source: string }

/**
 * The definitions in a library module. Each is written to stand alone, with a blank line between one
 * and the next; the comment above it is its description.
 */
export function parseLibrary(text: string, kind: Definition['kind']): Preset[] {
  const presets: Preset[] = [];
  for (const block of text.split(/\n\s*\n/)) {
    const lines = block.trim().split('\n'), start = lines.findIndex(line => new RegExp(`^${kind} \\w+`).test(line));
    if (start < 0) continue;
    const id = new RegExp(`^${kind} (\\w+)`).exec(lines[start])![1];
    const description = lines.slice(0, start).filter(line => line.startsWith('///')).map(line => line.slice(3).trim()).join(' ');
    presets.push({ id, name: id.charAt(0).toUpperCase() + id.slice(1).replaceAll('_', ' '), description, source: `${lines.join('\n')}\n` });
  }
  return presets;
}

export const instrumentPresets = parseLibrary(instruments, 'instr');
export const effectPresets = parseLibrary(fx, 'fx');
