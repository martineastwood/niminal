import { startValue, chanceValue, pitchValue, retriggerValue, glideRetriggers } from './project';

/** The sub-columns that hold what a note does, each with a popup editor. */
export type ExpressionKind = 'start' | 'chance' | 'pitch' | 'rtg';

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

/** A cell's text in plain words, for tooltips and the editor's preview. Null when it is not a valid cell. */
export function describe(kind: ExpressionKind, text: string): string | null {
  if (kind === 'start') {
    const value = startValue(text); if (value === null) return null;
    const parts = [/^\d/.test(value) ? `Starts ${parseInt(value, 10)}% into the sample` : '', value.endsWith('<') ? 'plays it backwards' : ''].filter(Boolean);
    return parts.join(' and ').replace(/^./, c => c.toUpperCase());
  }
  if (kind === 'chance') {
    const value = chanceValue(text); if (value === null) return null;
    return value === '100' ? 'Always plays' : value === '0' ? 'Never plays' : `Plays ${value}% of the time`;
  }
  if (kind === 'pitch') {
    const value = pitchValue(text); if (value === null) return null;
    const n = Number(value.slice(1));
    return value[0] === 'G' ? `Glides the held note to this one over ${n === 8 ? '1 row' : `${n / 8} rows`}` : `Slides the held note ${value[0] === 'U' ? 'up' : 'down'} ${plural(n, 'semitone')} over the row`;
  }
  const value = retriggerValue(text); if (value === null) return null;
  const [hits, change = '0'] = value.split(':'), c = Number(change);
  const tail = c === 0 ? '' : c === -100 ? ', then silent' : c < 0 ? `, each ${-c}% quieter than the one before` : `, each ${c}% louder than the one before, ending at the note's level`;
  return `Plays ${hits} times in the row${tail}`;
}

/** What `kind`'s popup asks for: a label, hint and range for each field, in order. */
interface Field { name: string; label: string; hint: string; type: 'number' | 'check' | 'choice'; choices?: [string, string][]; value: string }

/** The editor's fields for a cell's current text (or sensible blanks), and the text the fields make. */
function fields(kind: ExpressionKind, text: string | null): Field[] {
  if (kind === 'start') return [
    { name: 'offset', label: 'Offset', hint: '% into the sample, 0–99; blank starts at the beginning', type: 'number', value: /^\d+/.exec(text ?? '')?.[0] ?? '' },
    { name: 'reverse', label: 'Backwards', hint: 'plays the sample in reverse', type: 'check', value: text?.endsWith('<') ? '1' : '' },
  ];
  if (kind === 'chance') return [{ name: 'chance', label: 'Chance', hint: '% that the note plays, 0–100', type: 'number', value: text ?? '100' }];
  if (kind === 'pitch') return [
    { name: 'type', label: 'Movement', hint: '', type: 'choice', choices: [['G', 'Glide to this note'], ['U', 'Slide up'], ['D', 'Slide down']], value: text?.[0] ?? 'G' },
    { name: 'amount', label: 'Amount', hint: text?.[0] === 'U' || text?.[0] === 'D' ? 'semitones' : 'eighths of a row (8 is one row)', type: 'number', value: text?.slice(1) ?? '8' },
  ];
  const [hits = '4', change = ''] = (text ?? '').split(':');
  return [
    { name: 'hits', label: 'Hits', hint: 'times the note plays in its row, 2–32', type: 'number', value: hits },
    { name: 'change', label: 'Level change', hint: '% each hit differs from the one before, −100 to 100; blank is none', type: 'number', value: change },
  ];
}
function textOf(kind: ExpressionKind, values: Record<string, string>): string {
  if (kind === 'start') return values.offset.trim() + (values.reverse ? '<' : '');
  if (kind === 'chance') return values.chance.trim();
  if (kind === 'pitch') return values.type + values.amount.trim();
  const change = values.change.trim();
  return values.hits.trim() + (change === '' ? '' : `:${change}`);
}
const parseText = (kind: ExpressionKind, text: string) => ({ start: startValue, chance: chanceValue, pitch: pitchValue, rtg: retriggerValue })[kind](text);
const rival = (kind: ExpressionKind) => kind === 'pitch' ? 'retrigger' : 'glide';
const explanation: Record<ExpressionKind, string> = {
  start: 'Enter an offset from 0 to 99, or tick Backwards.',
  chance: 'Chance is a whole number from 0 to 100.',
  pitch: 'A glide takes 0–99 eighths of a row; a slide takes 1–99 semitones.',
  rtg: 'Hits run from 2 to 32 and the level change from −100 to 100.',
};

/**
 * Fills a popup panel with the editor for one cell. `other` is the text in the row's other competing
 * cell (the retrigger for a pitch cell, the pitch for a retrigger cell); `canReplace` says whether it can be
 * cleared along with this edit. `done` gets the new text (null clears the cell) and whether to clear the other cell.
 */
export function fillExpressionEditor(panel: HTMLElement, close: () => void, kind: ExpressionKind, current: string | null, other: string | null, canReplace: boolean, done: (value: string | null, clearOther: boolean) => void) {
  const title = document.createElement('strong'); title.textContent = { start: 'Start', chance: 'Chance', pitch: 'Pitch', rtg: 'Retrigger' }[kind]; panel.append(title);
  const inputs = new Map<string, HTMLInputElement | HTMLSelectElement>();
  for (const field of fields(kind, current)) {
    const label = document.createElement('label'); label.append(field.label);
    let input: HTMLInputElement | HTMLSelectElement;
    if (field.type === 'choice') {
      input = document.createElement('select');
      for (const [value, text] of field.choices!) input.add(new Option(text, value));
      input.value = field.value;
    } else {
      input = document.createElement('input'); input.type = field.type === 'check' ? 'checkbox' : 'text';
      if (field.type === 'check') input.checked = field.value !== ''; else { input.value = field.value; input.inputMode = 'numeric'; }
    }
    input.setAttribute('aria-label', field.label); inputs.set(field.name, input);
    const hint = document.createElement('small'); hint.textContent = field.hint; hint.dataset.field = field.name;
    label.append(input, hint); panel.append(label);
  }
  const preview = document.createElement('div'); preview.className = 'expression-preview';
  const problem = document.createElement('div'); problem.className = 'expression-problem'; problem.setAttribute('role', 'alert');
  const buttons = document.createElement('div'); buttons.className = 'expression-buttons';
  const button = (text: string, run: () => void) => { const b = document.createElement('button'); b.textContent = text; b.onclick = run; buttons.append(b); return b; };
  panel.append(preview, problem, buttons);

  const read = () => {
    const values = Object.fromEntries([...inputs].map(([name, input]) => [name, input instanceof HTMLInputElement && input.type === 'checkbox' ? (input.checked ? '1' : '') : input.value]));
    const text = textOf(kind, values), value = parseText(kind, text);
    return { text, value, conflict: value !== null && (kind === 'pitch' ? glideRetriggers(value, other) : kind === 'rtg' && glideRetriggers(other, value)) };
  };
  const update = () => {
    const { value, conflict } = read();
    if (kind === 'pitch') panel.querySelector<HTMLElement>('[data-field=amount]')!.textContent = (inputs.get('type') as HTMLSelectElement).value === 'G' ? 'eighths of a row (8 is one row)' : 'semitones';
    preview.textContent = value === null ? '' : `${value} — ${describe(kind, value)}`;
    problem.textContent = value === null ? explanation[kind] : conflict ? `A glide and a retrigger cannot share a row: this row already has the ${rival(kind)} ${other}.` : '';
    replace.hidden = !conflict || !canReplace; apply.disabled = value === null || conflict;
  };
  const finish = (clearOther: boolean) => { const { value, conflict } = read(); if (value === null || conflict && !clearOther) return; close(); done(value, clearOther); };
  const apply = button('Apply', () => finish(false));
  const replace = button(`Replace the ${rival(kind)}`, () => finish(true));
  button('Clear', () => { close(); done(null, false); });
  button('Cancel', close);
  for (const input of inputs.values()) {
    input.addEventListener('input', update); input.addEventListener('change', update);
    input.addEventListener('keydown', (event: Event) => { const key = event as KeyboardEvent; key.stopPropagation(); if (key.key === 'Enter' && !(key.target instanceof HTMLSelectElement)) { key.preventDefault(); if (!apply.disabled) finish(false); else if (!replace.hidden) finish(true); } });
  }
  update();
  inputs.values().next().value?.focus();
}
