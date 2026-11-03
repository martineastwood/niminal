import type { Definition } from './project';
import { readCuts } from './slices';
import { pieceKey } from './slices';

export interface Pad { member: number; gain: number; tune: number; choke: number }
export const folderKit = (source: string) => /^\w+\s*=\s*kit\("([^"\n]+)"\)/m.exec(source)?.[1];
const metadata = /^\/\/ tracker-pads: (.+)\n/m;
export function readPads(source: string, count: number): Pad[] {
  let saved: Partial<Pad>[] = [];
  try { saved = JSON.parse(metadata.exec(source)?.[1] ?? '[]'); } catch { /* plain source uses defaults */ }
  return Array.from({ length: count }, (_, i) => {
    const p = saved[i];
    return { member: Number.isInteger(p?.member) && p!.member! >= 0 && p!.member! < count ? p!.member! : i,
      gain: Number.isFinite(p?.gain) ? Math.max(-60, Math.min(12, p!.gain!)) : 0,
      tune: Number.isFinite(p?.tune) ? Math.max(-24, Math.min(24, p!.tune!)) : 0,
      choke: Number.isInteger(p?.choke) ? Math.max(0, Math.min(8, p!.choke!)) : 0 };
  });
}
export function clearPads(source: string): string {
  return source.replace(metadata, '').replace(/  \/\/ tracker-pad-controls\n[\s\S]*?  \/\/ end-pad-controls\n/, '')
    .replace(/pad_key: num = 0,\s*/, '').replaceAll('rate: rate * pad_rate', 'rate: rate').replace(/ \* pad_gain(?=\s*\n)/g, '')
    .replace(/\.choke\(pad_group_\d+\([^)]*\)\)/g, '');
}
/** Gain and tuning are language expressions; tracker notes supply the original key and mapped member. */
export function writePads(source: string, pads: Pad[], members: string[]): string {
  if (!pads.length || pads.length !== members.length) return source;
  const binding = /^(\w+)\s*=\s*(?:kit\("[^"\n]+"\)|\w+\.slices\([^\n]*?\))[^\n]*$/m.exec(source);
  if (!binding) return source;
  let text = clearPads(source);
  // Chokes belong to physical members; mapping keys does not change their identity.
  const groups = new Map<number, Set<string>>();
  pads.forEach(p => { if (p.choke) { const group = groups.get(p.choke) ?? new Set<string>(); group.add(members[p.member]); groups.set(p.choke, group); } });
  const line = binding[0].replace(/\.choke\(pad_group_\d+\([^)]*\)\)/g, '');
  const chokes = [...groups].map(([group, names]) => `.choke(pad_group_${group}(${[...names].join(', ')}))`).join('');
  text = text.replace(/^\w+\s*=\s*(?:kit\("[^"\n]+"\)|\w+\.slices\([^\n]*?\))[^\n]*$/m, line + chokes);
  const index = `pad_key.clamp(0..${pads.length - 1})`;
  const controls = `\n  // tracker-pad-controls\n  pad_rate = [${pads.map(p => Number((2 ** (p.tune / 12)).toFixed(8))).join(', ')}][${index}]\n  pad_gain = [${pads.map(p => Number((10 ** (p.gain / 20)).toFixed(8))).join(', ')}][${index}]\n  // end-pad-controls\n`;
  text = text.replace(/(instr\s+\w+\(\s*)/, '$1pad_key: num = 0,\n  ');
  text = text.replace(/(instr\s+\w+\([\s\S]*?\)\s*\{)\s*\n/, `$1${controls}`)
    .replaceAll('rate: rate', 'rate: rate * pad_rate');
  // Generated sampler instruments have one output expression, followed by the closing brace.
  text = text.replace(/([^\n]+)(\n\}\s*)$/, '$1 * pad_gain$2');
  return `// tracker-pads: ${JSON.stringify(pads)}\n${text}`;
}

export class SamplePads {
  readonly root = document.createElement('div');
  private revision = 0;
  constructor(private readonly actions: {
    edit(d: Definition, source: string): void;
    members?(folder: string): Promise<string[]>;
    hear(d: Definition, piece: number): void;
    replacePad?(folder: string, member: string): Promise<string | null>;
  }) { this.root.className = 'sample-pads'; }
  async refresh(definition: Definition | null, detected: number[] = []) {
    const revision = ++this.revision;
    const folder = definition && folderKit(definition.source);
    let members = definition ? (readCuts(definition.source)?.points ?? detected).map((_, i) => `s${i}`) : [];
    if (folder && this.actions.members) members = await this.actions.members(folder);
    if (revision !== this.revision) return;
    this.root.hidden = !definition || !members.length;
    if (!definition || !members.length) return;
    const pads = readPads(definition.source, members.length);
    const summary = document.createElement('h3'); summary.className = 'sample-heading'; summary.textContent = `${folder ? 'Kit key map' : 'Slice controls'} · ${members.length} keys`;
    const table = document.createElement('table');
    const head = document.createElement('tr'); for (const name of ['Key', 'Sample', 'dB', 'Semitones', 'Choke', '']) { const th = document.createElement('th'); th.textContent = name; head.append(th); } table.append(head);
    const commit = () => this.actions.edit(definition, writePads(definition.source, pads, members));
    pads.forEach((pad, index) => {
      const row = document.createElement('tr');
      const cell = (node: Node) => { const td = document.createElement('td'); td.append(node); row.append(td); };
      const hear = document.createElement('button'); hear.textContent = pieceKey(index); hear.onclick = () => this.actions.hear(definition, index); cell(hear);
      const sample = document.createElement('select'); sample.setAttribute('aria-label', `${pieceKey(index)} sample`);
      members.forEach((member, i) => { const o = document.createElement('option'); o.value = String(i); o.textContent = member; sample.append(o); }); sample.value = String(pad.member);
      sample.onchange = () => { pad.member = Number(sample.value); pad.choke = pads.find((p, i) => i !== index && p.member === pad.member)?.choke ?? pad.choke; commit(); }; cell(sample);
      for (const [key, min, max] of [['gain', -60, 12], ['tune', -24, 24], ['choke', 0, 8]] as const) {
        const input = document.createElement('input'); input.type = 'number'; input.min = String(min); input.max = String(max); input.step = key === 'choke' ? '1' : '0.1'; input.value = String(pad[key]); input.setAttribute('aria-label', `${pieceKey(index)} ${key}`);
        input.onchange = () => { const v = input.valueAsNumber; if (Number.isFinite(v) && v >= min && v <= max && (key !== 'choke' || Number.isInteger(v))) { pad[key] = v; if (key === 'choke') for (const other of pads) if (other.member === pad.member) other.choke = v; commit(); } else input.value = String(pad[key]); }; cell(input);
      }
      if (folder && this.actions.replacePad) {
        const replace = document.createElement('button'); replace.textContent = 'Replace…'; replace.onclick = async () => {
          replace.disabled = true;
          try { const next = await this.actions.replacePad!(folder, members[pad.member]); if (next && revision === this.revision) this.actions.edit(definition, definition.source.replace(`kit("${folder}")`, `kit("${next}")`)); }
          finally { replace.disabled = false; }
        }; cell(replace);
      }
      table.append(row);
    });
    const hint = document.createElement('p'); hint.className = 'device-note'; hint.textContent = 'Choke 0 is off. Samples in the same group cut one another off; groups follow the sample when keys are remapped.';
    this.root.replaceChildren(summary, hint, table);
  }
}
