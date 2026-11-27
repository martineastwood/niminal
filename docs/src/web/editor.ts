// The code editor the web pages share: the language's own highlighting in the site's two grounds, the
// language server's help, Run keys, and a flash over the code that has just been run.
import { basicSetup, EditorView } from 'codemirror';
import { Compartment, EditorState, Prec, StateEffect, StateField } from '@codemirror/state';
import { Decoration, type DecorationSet, keymap } from '@codemirror/view';
import { indentWithTab } from '@codemirror/commands';
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { lintGutter } from '@codemirror/lint';
import { acceptCompletion } from '@codemirror/autocomplete';
import { tags } from '@lezer/highlight';
import DOMPurify from 'dompurify';
// the same lexer, palette and language-server client the Stage editor uses
import { colours, niminal } from '../../../editors/codemirror/language';
import { connectLanguageServer, languageClient } from '../../../editors/codemirror/lsp';
import { semanticHighlighting } from '../../../editors/codemirror/semantic';
import { blockAround } from '../../../editors/codemirror/blocks';

const paperColours = syntaxHighlighting(HighlightStyle.define([
  { tag: tags.keyword, color: '#8250df' },
  { tag: tags.comment, color: '#6e7781' },
  { tag: tags.string, color: '#1a7f37' },
  { tag: [tags.number, tags.atom], color: '#b35900' },
  { tag: tags.typeName, color: '#0a7e7e' },
  { tag: tags.className, color: '#1a7f37' },
  { tag: tags.function(tags.variableName), color: '#0550ae' },
  { tag: tags.propertyName, color: '#a5303d' },
  { tag: tags.local(tags.variableName), color: '#a5303d' },
  { tag: tags.definition(tags.variableName), color: '#9a6700' },
  { tag: tags.special(tags.variableName), color: '#953800' },
  { tag: tags.operator, color: '#57606a' },
  { tag: [tags.punctuation, tags.bracket], color: '#57606a' },
]));
const editorTheme = (dark: boolean) => EditorView.theme({
  '&': { backgroundColor: 'var(--panel)', color: 'var(--panel-ink)', fontSize: '0.9rem' },
  '.cm-content': { fontFamily: 'var(--mono)', caretColor: 'var(--accent)', padding: '0.6rem 0' },
  '.cm-gutters': { backgroundColor: 'var(--panel-bar)', color: 'var(--panel-muted)', border: 'none', borderRight: '1px solid var(--panel-line)', fontFamily: 'var(--mono)' },
  '.cm-activeLine': { backgroundColor: 'color-mix(in srgb, var(--accent) 7%, transparent)' },
  '.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--panel-ink)' },
  '.cm-cursor': { borderLeftColor: 'var(--accent)' },
  '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground': { backgroundColor: 'color-mix(in srgb, var(--accent) 28%, transparent)' },
  '.cm-matchingBracket': { outline: '1px solid var(--accent)', backgroundColor: 'transparent' },
  '.cm-tooltip': { backgroundColor: 'var(--bg-elevated)', color: 'var(--ink)', border: '1px solid var(--line-strong)', borderRadius: '4px', maxWidth: '36rem' },
  '.cm-tooltip pre': { whiteSpace: 'pre-wrap', fontFamily: 'var(--mono)' },
  '.cm-tooltip-autocomplete > ul > li[aria-selected]': { backgroundColor: 'var(--accent)', color: 'var(--accent-ink)' },
}, { dark });
// the compiler's colours for what it has resolved: Stage's are for the ink ground, so paper gets its own
const paperSemantic = Prec.high(EditorView.theme({
  '.cm-sem-function': { color: '#0550ae' },
  '.cm-sem-class, .cm-sem-namespace': { color: '#0a7e7e' },
  '.cm-sem-variable': { color: '#9a6700' },
  '.cm-sem-parameter': { color: '#a5303d' },
  '.cm-sem-enumMember': { color: '#b35900' },
  '.cm-sem-enum': { color: '#1a7f37' },
}));

const flashOn = StateEffect.define<{ from: number; to: number }>();
const flashOff = StateEffect.define<null>();
const flashField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    value = value.map(tr.changes);
    for (const e of tr.effects) {
      if (e.is(flashOn)) value = Decoration.set([Decoration.mark({ class: 'cm-flash' }).range(e.value.from, e.value.to)]);
      if (e.is(flashOff)) value = Decoration.none;
    }
    return value;
  },
  provide: (f) => EditorView.decorations.from(f),
});

export type CodeEditor = ReturnType<typeof createEditor>;

export function createEditor(options: {
  parent: HTMLElement;
  /** the language server's connection: `engine.exchange` */
  exchange: (message: Record<string, unknown>) => Promise<Record<string, unknown>[]>;
  uri?: string;
  /** Mod-Enter */
  onRun: () => void;
  /** Mod-Shift-Enter */
  onRunAll: () => void;
}) {
  const root = document.documentElement;
  const lsp = languageClient((html) => DOMPurify.sanitize(html));
  const ground = new Compartment();
  const groundExtensions = () => {
    const dark = root.dataset.mode !== 'paper';
    return [editorTheme(dark), dark ? colours : [paperColours, paperSemantic]];
  };
  const view = new EditorView({
    parent: options.parent,
    state: EditorState.create({
      doc: '',
      extensions: [
        Prec.highest(keymap.of([
          { key: 'Mod-Shift-Enter', run: () => { options.onRunAll(); return true; } },
          { key: 'Mod-Enter', run: () => { options.onRun(); return true; } },
        ])),
        flashField,
        basicSetup,
        // Tab takes the suggestion when the list is open, and indents otherwise
        keymap.of([{ key: 'Tab', run: acceptCompletion }, indentWithTab]),
        niminal,
        semanticHighlighting,
        // mistakes, completion and hovers come from the language server, running in a worker
        lsp.plugin(options.uri ?? 'file:///studio.nml', 'niminal'),
        lintGutter(),
        ground.of(groundExtensions()),
      ],
    }),
  });
  void connectLanguageServer(lsp, options.exchange, (message) => console.info(message));

  let flashTimer: number | undefined;
  return {
    view,
    get value() { return view.state.doc.toString(); },
    set value(text: string) { view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } }); },
    select(from: number, to: number) { view.focus(); view.dispatch({ selection: { anchor: from, head: to }, scrollIntoView: true }); },
    /** Follow the page's ink or paper ground. */
    applyGround() { view.dispatch({ effects: ground.reconfigure(groundExtensions()) }); },
    /** Mark code as just run, for a moment. */
    flash(from: number, to: number) {
      if (to <= from) return;
      view.dispatch({ effects: flashOn.of({ from, to }) });
      clearTimeout(flashTimer);
      flashTimer = window.setTimeout(() => view.dispatch({ effects: flashOff.of(null) }), 450);
    },
    /** The selection, or the block the cursor is in: the code Run sends. Null between blocks. */
    target() {
      const selection = view.state.selection.main;
      if (!selection.empty) return { from: selection.from, to: selection.to };
      const line = view.state.doc.lineAt(selection.head);
      const hit = blockAround(view.state.doc.toString().split('\n'), line.number - 1);
      if (!hit) return null;
      return { from: view.state.doc.line(hit.start + 1).from, to: view.state.doc.line(hit.end + 1).to };
    },
  };
}
