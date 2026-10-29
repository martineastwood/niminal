import { StateEffect, StateField, Text } from '@codemirror/state';
import { Decoration, DecorationSet, EditorView, ViewPlugin, ViewUpdate } from '@codemirror/view';
import { LSPPlugin } from '@codemirror/lsp-client';

/** The legend the server sends, mirrored here so a name resolves to a colour class. */
const KINDS = ['function', 'class', 'variable', 'parameter', 'enumMember', 'namespace', 'enum'];

const marks = Object.fromEntries(KINDS.map(kind => [kind, Decoration.mark({ class: `cm-sem-${kind}` })]));

/** Turn the server's delta-encoded tokens (line, column, length, type, modifiers) into marks. */
export function semanticMarks(doc: Text, data: number[]): DecorationSet {
  const ranges = [];
  let line = 0;
  let column = 0;
  for (let i = 0; i + 4 < data.length; i += 5) {
    line += data[i];
    column = data[i] === 0 ? column + data[i + 1] : data[i + 1];
    const mark = marks[KINDS[data[i + 3]]];
    if (!mark || line >= doc.lines) continue;
    const from = doc.line(line + 1).from + column;
    ranges.push(mark.range(from, from + data[i + 2]));
  }
  return Decoration.set(ranges);
}

const received = StateEffect.define<DecorationSet>();

/** Marks follow their words through edits until the server's next answer replaces them. */
const field = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    for (const effect of tr.effects) if (effect.is(received)) return effect.value;
    return tr.docChanged ? value.map(tr.changes) : value;
  },
  provide: f => EditorView.decorations.from(f),
});

/** How long to wait after typing stops before asking for the colours again. */
const SETTLE = 150;

const asking = ViewPlugin.fromClass(
  class {
    timer: ReturnType<typeof setTimeout> | undefined;
    constructor(readonly view: EditorView) {
      // a client that connects after the editor opens colours it once it is ready
      LSPPlugin.get(view)?.client.initializing.then(() => this.schedule(), () => {});
    }
    update(update: ViewUpdate) {
      if (update.docChanged) this.schedule();
    }
    schedule() {
      clearTimeout(this.timer);
      this.timer = setTimeout(() => this.ask(), SETTLE);
    }
    async ask() {
      const plugin = LSPPlugin.get(this.view);
      if (!plugin?.client.connected || !plugin.client.serverCapabilities?.semanticTokensProvider) return;
      const asked = this.view.state.doc;
      try {
        plugin.client.sync();
        const reply = await plugin.client.request<unknown, { data: number[] }>('textDocument/semanticTokens/full', {
          textDocument: { uri: plugin.uri },
        });
        // the answer describes the text that was sent; a newer edit asks again
        if (this.view.state.doc === asked) this.view.dispatch({ effects: received.of(semanticMarks(asked, reply.data)) });
      } catch {
        // the lexical colours stay until the server answers
      }
    }
    destroy() {
      clearTimeout(this.timer);
    }
  },
);

/** Colours of the names the compiler resolves, matching the palette of the lexical colours. */
export const semanticTheme = EditorView.theme({
  '.cm-sem-function': { color: '#8cbce7' },
  '.cm-sem-class, .cm-sem-namespace': { color: '#74c7c3' },
  '.cm-sem-variable': { color: '#e5c07b' },
  '.cm-sem-parameter': { color: '#e8a0a8' },
  '.cm-sem-enumMember': { color: '#e9b66e' },
  '.cm-sem-enum': { color: '#a6d189' },
});

export const semanticHighlighting = [field, asking, semanticTheme];
