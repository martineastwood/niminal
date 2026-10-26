import { ChangeSet, StateEffect, StateField } from '@codemirror/state';
import { Decoration, DecorationSet, EditorView } from '@codemirror/view';
import { Problem, Snapshot, sinceOnset } from './live';
import { sceneStates } from './scenes';

/** How long a note stays lit after it starts, in seconds. */
const DECAY = 0.45;
/** Brightness steps; each is a CSS class, so a frame only rebuilds when a step changes. */
const STEPS = 6;
/** One colour per clip, so the code, the lane and the meter read as the same thing. */
export const CLIP_COLOURS = ['#e9b66e', '#8cbce7', '#c6a1e8', '#a5c98d', '#e08b8b', '#74c7c3'];

type Evaluated = {
  /** The number the stage gave this evaluation, which the engine reports back with each note. */
  id: number;
  /** Where the evaluated text begins in the document: the selection start, or 0. */
  base: number;
  text: string;
};

/** Marks that the editor's text was just sent to the engine, so the engine's byte offsets apply to it. */
export const evaluated = StateEffect.define<Evaluated>();
const lit = StateEffect.define<DecorationSet>();

type Drift = {
  base: number;
  /** Byte offset in the evaluated text to its position in the editor. */
  unit: (byte: number) => number;
  /** How much text the evaluation sent, so a range outside it is ignored. */
  length: number;
  /** Edits made since the evaluation, so a highlight follows its note as the text moves. */
  changes: ChangeSet;
};

function unitMap(text: string): (byte: number) => number {
  const bytes = new TextEncoder().encode(text);
  if (bytes.length === text.length) return byte => byte;
  // multi-byte characters: map each byte to the UTF-16 index of the character it belongs to
  const map = new Uint32Array(bytes.length + 1);
  let byte = 0;
  let unit = 0;
  for (const char of text) {
    const size = new TextEncoder().encode(char).length;
    for (let i = 0; i < size; i++) map[byte + i] = unit;
    byte += size;
    unit += char.length;
  }
  map[byte] = unit;
  return b => map[Math.min(b, bytes.length)];
}

/** How many evaluations to remember: a clip keeps the text of whichever one last defined it. */
const REMEMBERED = 32;

const drift = StateField.define<Map<number, Drift>>({
  create: () => new Map(),
  update(value, tr) {
    let next = value;
    if (tr.docChanged) {
      next = new Map();
      for (const [id, d] of value) next.set(id, { ...d, changes: d.changes.compose(tr.changes) });
    }
    for (const effect of tr.effects) {
      if (!effect.is(evaluated)) continue;
      if (next === value) next = new Map(value);
      next.set(effect.value.id, {
        base: effect.value.base,
        unit: unitMap(effect.value.text),
        length: new TextEncoder().encode(effect.value.text).length,
        changes: ChangeSet.empty(tr.state.doc.length),
      });
      for (const id of next.keys()) {
        if (next.size > REMEMBERED) next.delete(id);
        else break;
      }
    }
    return next;
  },
});

const hits = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    for (const effect of tr.effects) if (effect.is(lit)) return effect.value;
    return tr.docChanged ? value.map(tr.changes) : value;
  },
  provide: field => EditorView.decorations.from(field),
});

/** Lines of code just evaluated, shown briefly so it is clear what was sent. */
const flashed = StateEffect.define<DecorationSet>();
const flash = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    for (const effect of tr.effects) if (effect.is(flashed)) return effect.value;
    return tr.docChanged ? value.map(tr.changes) : value;
  },
  provide: field => EditorView.decorations.from(field),
});

/** The `clip` and `scene` header lines, marked with whether they are playing, muted or waiting to start. */
const marked = StateEffect.define<DecorationSet>();
const headers = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    for (const effect of tr.effects) if (effect.is(marked)) return effect.value;
    return tr.docChanged ? value.map(tr.changes) : value;
  },
  provide: field => EditorView.decorations.from(field),
});

/** Where the last evaluation went wrong, underlined until the next one. */
const refused = StateEffect.define<DecorationSet>();
const refusals = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    for (const effect of tr.effects) {
      if (effect.is(refused)) return effect.value;
      if (effect.is(evaluated)) return Decoration.none;
    }
    return tr.docChanged ? value.map(tr.changes) : value;
  },
  provide: field => EditorView.decorations.from(field),
});

type Sent = { from: number; to: number; landed: boolean };
const sentEval = StateEffect.define<{ id: number; from: number; to: number }>();
const settledEval = StateEffect.define<{ id: number; landed: boolean | null }>();

/**
 * The lines of each evaluation that was accepted but has not landed: marked until the engine makes the
 * change, then shown landed for a moment. `landed: null` forgets one (cancelled, or the flash is over).
 */
const sentLines = StateField.define<Map<number, Sent>>({
  create: () => new Map(),
  update(value, tr) {
    let next = value;
    if (tr.docChanged) {
      next = new Map();
      for (const [id, s] of value) next.set(id, { ...s, from: tr.changes.mapPos(s.from, 1), to: tr.changes.mapPos(s.to, -1) });
    }
    for (const effect of tr.effects) {
      if (effect.is(sentEval)) {
        if (next === value) next = new Map(value);
        next.set(effect.value.id, { from: effect.value.from, to: effect.value.to, landed: false });
      } else if (effect.is(settledEval)) {
        const entry = next.get(effect.value.id);
        if (!entry) continue;
        if (next === value) next = new Map(value);
        if (effect.value.landed === null) next.delete(effect.value.id);
        else next.set(effect.value.id, { ...entry, landed: effect.value.landed });
      }
    }
    return next;
  },
  provide: field =>
    EditorView.decorations.compute([field], state => {
      const marks = new Map<number, string>();
      for (const s of state.field(field).values()) {
        const doc = state.doc;
        const last = Math.min(s.to, doc.length);
        for (let line = doc.lineAt(Math.min(s.from, doc.length)); ; line = doc.line(line.number + 1)) {
          // landed wins over waiting where two evaluations share a line
          if (marks.get(line.from) !== 'cm-landed') marks.set(line.from, s.landed ? 'cm-landed' : 'cm-queued-eval');
          if (line.to >= last || line.number >= doc.lines) break;
        }
      }
      return Decoration.set([...marks].sort((a, b) => a[0] - b[0]).map(([from, cls]) => Decoration.line({ class: cls }).range(from)));
    }),
});

export const highlightExtensions = [drift, hits, flash, headers, refusals, sentLines];

/** Mark the lines `from` to `to` as sent in pending change `id`, until it lands. */
export function markSent(view: EditorView, id: number, from: number, to: number) {
  view.dispatch({ effects: sentEval.of({ id, from, to }) });
}

/** How long a landed change stays marked green, in milliseconds. */
const LANDED_FOR = 900;

/** Underline what each problem points at in the text that was sent from `base`. */
export function markProblems(view: EditorView, base: number, text: string, problems: Problem[]) {
  const lines = text.split('\n');
  const marks = [];
  for (const problem of problems) {
    if (!problem.line || problem.line > lines.length) continue;
    const before = lines.slice(0, problem.line - 1).reduce((n, line) => n + line.length + 1, 0);
    const line = lines[problem.line - 1];
    const column = Math.min(Math.max((problem.column ?? 1) - 1, 0), line.length);
    const word = /^[\w.:]+|^\S/.exec(line.slice(column));
    const from = base + before + column;
    const to = from + (word ? word[0].length : Math.max(line.length - column, 1));
    if (to > view.state.doc.length) continue;
    marks.push(Decoration.mark({ class: 'cm-refused', attributes: { title: problem.message } }).range(from, to));
  }
  view.dispatch({ effects: refused.of(Decoration.set(marks, true)) });
}

/** Flash the lines from `from` to `to`, which fade on their own and are then taken off. */
export function flashLines(view: EditorView, from: number, to: number) {
  const marks = [];
  for (let line = view.state.doc.lineAt(from); ; line = view.state.doc.line(line.number + 1)) {
    marks.push(Decoration.line({ class: 'cm-flash' }).range(line.from));
    if (line.to >= to || line.number >= view.state.doc.lines) break;
  }
  view.dispatch({ effects: flashed.of(Decoration.set(marks)) });
  setTimeout(() => view.dispatch({ effects: flashed.of(Decoration.none) }), 700);
}

/** The ranges lit now, for tests. */
export function litRanges(view: EditorView) {
  const out: { from: number; to: number; text: string; class: string }[] = [];
  const cursor = view.state.field(hits).iter();
  for (; cursor.value; cursor.next()) {
    out.push({
      from: cursor.from,
      to: cursor.to,
      text: view.state.sliceDoc(cursor.from, cursor.to),
      class: String(cursor.value.spec.class),
    });
  }
  return out;
}

/** Lights the source of each note as it plays, in the order the clip plays them. */
export class Highlighter {
  private snapshot: Snapshot | null = null;
  private updatedAt = 0;
  private shown = '';
  private headed = '';
  private headedDoc: unknown = null;

  constructor(private view: () => EditorView | undefined) {
    const frame = () => {
      this.paint();
      requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
  }

  /** The editor now shows another tab: forget what was drawn so the next frame draws this one's. */
  reset() {
    this.shown = '';
    this.headed = '';
  }

  update(snapshot: Snapshot) {
    this.snapshot = snapshot;
    this.updatedAt = performance.now();
    this.settle(snapshot);
  }

  /** Turn the lines of each evaluation that landed green, and forget those that are no longer waiting. */
  private settle(snapshot: Snapshot) {
    const view = this.view();
    if (!view) return;
    const waiting = new Set(snapshot.pending.map(p => p.id));
    const landed = new Set(snapshot.landed.map(l => l.id));
    for (const [id, sent] of view.state.field(sentLines)) {
      if (sent.landed || waiting.has(id)) continue;
      if (landed.has(id)) {
        view.dispatch({ effects: settledEval.of({ id, landed: true }) });
        setTimeout(() => view.dispatch({ effects: settledEval.of({ id, landed: null }) }), LANDED_FOR);
      } else {
        view.dispatch({ effects: settledEval.of({ id, landed: null }) });
      }
    }
  }

  paint() {
    try {
      this.light();
    } catch (error) {
      // one bad range must not end the frame loop, or nothing would light again
      console.error('lighting the notes failed', error);
    }
  }

  /** Mark each `clip name` and `scene name` line with what the clip or scene is doing. */
  private mark(view: EditorView, snapshot: Snapshot) {
    const states = new Map<string, { kind: string; classes: string; colour: string }>();
    const waiting = (name: string) => snapshot.pending.some(p => p.changes.some(c => c.startsWith(`${name}.`)));
    snapshot.clips.forEach((clip, index) => {
      const classes = [
        clip.running && !clip.muted ? 'cm-live-on' : '',
        clip.muted ? 'cm-live-muted' : '',
        clip.soloed ? 'cm-live-solo' : '',
        waiting(clip.name) ? 'cm-live-queued' : '',
      ].filter(Boolean);
      if (classes.length) {
        states.set(`clip ${clip.name}`, { kind: 'clip', classes: classes.join(' '), colour: CLIP_COLOURS[index % CLIP_COLOURS.length] });
      }
    });
    const scenes = sceneStates(snapshot.scenes);
    for (const scene of snapshot.scenes) {
      const state = scenes.get(scene.name)?.state;
      const classes = [
        state === 'playing' ? 'cm-live-on' : state === 'included' || state === 'partial' ? 'cm-live-part' : '',
        waiting(scene.name) ? 'cm-live-queued' : '',
      ].filter(Boolean);
      if (classes.length) states.set(`scene ${scene.name}`, { kind: 'scene', classes: classes.join(' '), colour: '#e9b66e' });
    }
    const signature = JSON.stringify([...states]);
    if (signature === this.headed && view.state.doc === this.headedDoc) return;
    this.headed = signature;
    this.headedDoc = view.state.doc;
    const marks = [];
    for (let n = 1; n <= view.state.doc.lines; n++) {
      const line = view.state.doc.line(n);
      const header = /^\s*(clip|scene)\s+([A-Za-z_]\w*)/.exec(line.text);
      const state = header && states.get(`${header[1]} ${header[2]}`);
      if (state) {
        marks.push(Decoration.line({ class: `cm-live ${state.classes}`, attributes: { style: `--hit:${state.colour}` } }).range(line.from));
      }
    }
    view.dispatch({ effects: marked.of(Decoration.set(marks)) });
  }

  private light() {
    const view = this.view();
    const snapshot = this.snapshot;
    if (!view || !snapshot?.transport) return;
    this.mark(view, snapshot);
    const beats = snapshot.beats_per_bar || 4;
    const secondsPerCycle = (60 / snapshot.transport.bpm) * beats;
    const elapsed = (performance.now() - this.updatedAt) / 1000;
    const evals = view.state.field(drift);
    const ranges: { from: number; to: number; step: number; colour: string }[] = [];

    snapshot.clips.forEach((clip, index) => {
      if (!clip.running || clip.cycles == null) return;
      const now = clip.cycles + elapsed / secondsPerCycle;
      for (const lane of clip.lanes) {
        for (const event of lane.events) {
          const text = event.eval == null ? undefined : evals.get(event.eval);
          if (!event.src || !text) continue;
          const { base, unit, changes, length } = text;
          if (event.src[1] > length) continue;
          const age = sinceOnset(clip, event, now);
          if (age === null) continue;
          const seconds = age * secondsPerCycle;
          if (seconds >= DECAY) continue;
          const from = changes.mapPos(base + unit(event.src[0]), 1);
          const to = changes.mapPos(base + unit(event.src[1]), -1);
          if (to <= from || to > view.state.doc.length) continue;
          const step = Math.max(1, Math.ceil((1 - seconds / DECAY) * STEPS));
          ranges.push({ from, to, step, colour: CLIP_COLOURS[index % CLIP_COLOURS.length] });
        }
      }
    });

    ranges.sort((a, b) => a.from - b.from || a.to - b.to);
    const signature = ranges.map(r => `${r.from}:${r.to}:${r.step}:${r.colour}`).join(',');
    if (signature === this.shown) return;
    this.shown = signature;
    const marks = ranges.map(r =>
      Decoration.mark({
        class: `cm-hit cm-hit-${r.step}`,
        attributes: { style: `--hit:${r.colour}` },
      }).range(r.from, r.to),
    );
    view.dispatch({ effects: lit.of(Decoration.set(marks, true)) });
  }
}
