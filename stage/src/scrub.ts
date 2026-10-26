import { Prec } from '@codemirror/state';
import { EditorView, keymap } from '@codemirror/view';

/** A number written in the code: its digits (with a minus sign if it has one), and the unit after them. */
export type NumberAt = { from: number; to: number; end: number };

/**
 * The number under `column` in `line`, or failing that the first one after it. `to` is the end of the digits
 * and `end` the end of the unit that follows (`440hz`), which is part of the target so the unit can be grabbed.
 * Names such as `c4` or `kick2` and anything in a `//` comment are not numbers.
 */
export function numberAt(line: string, column: number): NumberAt | null {
  const comment = line.indexOf('//');
  const found: NumberAt[] = [];
  for (const match of line.matchAll(/(?<![\w.])\d+(?:\.\d+)?/g)) {
    if (comment >= 0 && match.index >= comment) break;
    let from = match.index;
    // `-6db` is one number, but `a-1` is a subtraction
    if (line[from - 1] === '-' && !/[\w)\]]/.test(line[from - 2] ?? ' ')) from -= 1;
    const to = match.index + match[0].length;
    found.push({ from, to, end: to + (/^[a-z%]*/i.exec(line.slice(to))?.[0].length ?? 0) });
  }
  return found.find(n => column >= n.from && column <= n.end) ?? found.find(n => n.from >= column) ?? null;
}

/**
 * `text` moved by `steps` steps. A step is the last digit it is written to, scaled up for big numbers so a
 * drag across 8000hz does not take for ever (440 moves by 1, 4400 by 10). Decimals are kept as written.
 */
export function nudge(text: string, steps: number) {
  const value = Number(text);
  const decimals = text.split('.')[1]?.length ?? 0;
  const magnitude = Math.abs(value) >= 1 ? 10 ** (Math.floor(Math.log10(Math.abs(value))) - 2) : 0;
  const step = Math.max(10 ** -decimals, magnitude);
  const next = Math.round((value + steps * step) / 10 ** -decimals) * 10 ** -decimals;
  const out = next.toFixed(decimals);
  return /^-0(\.0*)?$/.test(out) ? out.slice(1) : out;
}

/** Pixels of drag per step. */
const PIXELS = 3;

/**
 * Alt-drag a number sideways to change it, and let go to send the block it is in: so the number and the
 * moment are both yours (Esc while dragging puts it back). Cmd/Ctrl+Alt+↑↓ with the cursor in a number
 * nudges it (Shift for tens) and leaves sending to you.
 */
export function scrubbing(send: () => void) {
  /** Replace the number at `column` of the line holding `pos`, and put the cursor in it. */
  const set = (view: EditorView, from: number, to: number, text: string) => {
    view.dispatch({ changes: { from, to, insert: text }, selection: { anchor: from + text.length }, userEvent: 'input.scrub' });
  };

  const find = (view: EditorView, pos: number) => {
    const line = view.state.doc.lineAt(pos);
    const hit = numberAt(line.text, pos - line.from);
    return hit && { from: line.from + hit.from, to: line.from + hit.to, end: line.from + hit.end, pos };
  };

  const key = (direction: 1 | -1, size: number) => (view: EditorView) => {
    const hit = find(view, view.state.selection.main.head);
    if (!hit) return false;
    set(view, hit.from, hit.to, nudge(view.state.sliceDoc(hit.from, hit.to), direction * size));
    return true;
  };

  return Prec.highest([
    keymap.of([
      { key: 'Mod-Alt-ArrowUp', run: key(1, 1) },
      { key: 'Mod-Alt-ArrowDown', run: key(-1, 1) },
      { key: 'Mod-Alt-Shift-ArrowUp', run: key(1, 10) },
      { key: 'Mod-Alt-Shift-ArrowDown', run: key(-1, 10) },
    ]),
    EditorView.domEventHandlers({
      mousemove(event, view) {
        const over = event.altKey && !event.buttons && (() => {
          const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
          return pos !== null && find(view, pos) !== null;
        })();
        view.contentDOM.style.cursor = over ? 'ew-resize' : '';
      },
      mousedown(event, view) {
        if (!event.altKey || event.button !== 0) return false;
        const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
        const hit = pos === null ? null : find(view, pos);
        if (!hit) return false;
        event.preventDefault();
        const start = event.clientX;
        const original = view.state.sliceDoc(hit.from, hit.to);
        let to = hit.to;
        const move = (e: MouseEvent) => {
          const steps = Math.round((e.clientX - start) / PIXELS) * (e.shiftKey ? 10 : 1);
          const text = nudge(original, steps);
          if (text === view.state.sliceDoc(hit.from, to)) return;
          set(view, hit.from, to, text);
          to = hit.from + text.length;
        };
        const finish = (keep: boolean) => {
          window.removeEventListener('mousemove', move);
          window.removeEventListener('mouseup', up);
          window.removeEventListener('keydown', escape, true);
          document.body.style.cursor = '';
          if (!keep) set(view, hit.from, to, original);
          else if (view.state.sliceDoc(hit.from, to) !== original) send();
        };
        const up = () => finish(true);
        const escape = (e: KeyboardEvent) => {
          if (e.key !== 'Escape') return;
          e.preventDefault();
          finish(false);
        };
        document.body.style.cursor = 'ew-resize';
        window.addEventListener('mousemove', move);
        window.addEventListener('mouseup', up);
        window.addEventListener('keydown', escape, true);
        return true;
      },
    }),
  ]);
}
