import catalog from '../../crates/niminal-lang/std/presets.json';

/**
 * One ready-made instrument or effect from the catalog that comes with niminal (`niminal presets` lists the
 * same one). The catalog is written by niminal-lang from its `std/` shelves; this file only reads it.
 */
export interface Preset {
  id: string; name: string; description: string; source: string;
  /** Where it sits in the browser, like `drums/kicks`. */
  folder: string;
  /** The functions it calls that its shelf defines (the `drum` voice of a drum preset), copied into the project along with it. */
  helpers: string;
}

const presets = catalog as (Preset & { kind: 'instr' | 'fx' })[];

export const instrumentPresets = presets.filter(p => p.kind === 'instr');
export const effectPresets = presets.filter(p => p.kind === 'fx');
