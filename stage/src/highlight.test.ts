// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { Highlighter, evaluated, highlightExtensions, litRanges } from './highlight';
import { Clip, Snapshot } from './live';

const source = 'clip a { notes: [c4 e4] }';

function clip(events: { start: number; value: string; src: [number, number]; eval: number }[]): Clip {
  return {
    name: 'a',
    instrument: 'pluck',
    running: true,
    muted: false,
    soloed: false,
    edits: [],
    length: 1,
    looped: true,
    endless: false,
    cycles: 0,
    lanes: [
      {
        name: 'notes',
        truncated: false,
        events: events.map(e => ({ dur: 0.5, gain: null, pan: null, ...e })),
      },
    ],
  };
}

function snapshot(clips: Clip[]): Snapshot {
  return {
    connected: true,
    transport: { bpm: 120 },
    beats_per_bar: 4,
    clips,
  } as Snapshot;
}

function setup(doc = source) {
  const view = new EditorView({ state: EditorState.create({ doc, extensions: highlightExtensions }) });
  return { view, highlighter: new Highlighter(() => view) };
}

describe('lighting notes in the source', () => {
  it('lights the note that starts now, at the text it was written', () => {
    const { view, highlighter } = setup();
    view.dispatch({ effects: evaluated.of({ id: 1, base: 0, text: source }) });
    highlighter.update(snapshot([clip([
      { start: 0, value: 'c4', src: [source.indexOf('c4'), source.indexOf('c4') + 2], eval: 1 },
      { start: 0.5, value: 'e4', src: [source.indexOf('e4'), source.indexOf('e4') + 2], eval: 1 },
    ])]));
    highlighter.paint();
    expect(litRanges(view).map(r => r.text)).toEqual(['c4']);
  });

  it('follows the text when it is edited after the evaluation', () => {
    const { view, highlighter } = setup();
    view.dispatch({ effects: evaluated.of({ id: 1, base: 0, text: source }) });
    view.dispatch({ changes: { from: 0, insert: '// riff\n' } });
    highlighter.update(snapshot([clip([
      { start: 0, value: 'c4', src: [source.indexOf('c4'), source.indexOf('c4') + 2], eval: 1 },
    ])]));
    highlighter.paint();
    expect(litRanges(view).map(r => r.text)).toEqual(['c4']);
  });

  it('counts from the start of a selection that was evaluated alone', () => {
    const { view, highlighter } = setup(`// intro\n${source}`);
    const selected = source;
    view.dispatch({ effects: evaluated.of({ id: 2, base: 9, text: selected }) });
    highlighter.update(snapshot([clip([
      { start: 0, value: 'c4', src: [selected.indexOf('c4'), selected.indexOf('c4') + 2], eval: 2 },
    ])]));
    highlighter.paint();
    expect(litRanges(view).map(r => r.text)).toEqual(['c4']);
  });

  it('lights nothing for an evaluation it does not know', () => {
    const { view, highlighter } = setup();
    highlighter.update(snapshot([clip([{ start: 0, value: 'c4', src: [16, 18], eval: 7 }])]));
    highlighter.paint();
    expect(litRanges(view)).toEqual([]);
  });

  it('keeps going when the engine reports a place beyond the text it was sent', () => {
    // what an engine kept from an earlier run of the app can carry the same number as a new evaluation
    const { view, highlighter } = setup();
    view.dispatch({ effects: evaluated.of({ id: 1, base: 0, text: source }) });
    highlighter.update(snapshot([clip([{ start: 0, value: 'c4', src: [5000, 5002], eval: 1 }])]));
    expect(() => highlighter.paint()).not.toThrow();
    expect(litRanges(view)).toEqual([]);
  });
});
