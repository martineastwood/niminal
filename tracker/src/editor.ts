import { basicSetup } from 'codemirror';
import { EditorState, Prec } from '@codemirror/state';
import { EditorView, keymap } from '@codemirror/view';
import { acceptCompletion } from '@codemirror/autocomplete';
import { indentWithTab } from '@codemirror/commands';
import { invoke, isTauri } from '@tauri-apps/api/core';
import DOMPurify from 'dompurify';
import { colours, niminal } from '../../editors/codemirror/language';
import { semanticHighlighting } from '../../editors/codemirror/semantic';
import { connectLanguageServer as connect, editorTheme, languageClient } from '../../editors/codemirror/lsp';

export const lsp = languageClient(html => DOMPurify.sanitize(html));

export async function connectLanguageServer(status: (message: string) => void) {
  if (!isTauri()) { status('Browser preview · open the desktop app for the language server'); return; }
  await connect(lsp, message => invoke('lsp_message', { message }), status);
}

const theme = editorTheme('var(--code-font, 12px)');
/** `apply` runs on Cmd/Ctrl+Enter, which stands in for the editor's own "insert blank line". */
export function editorState(uri: string, source: string, changed: (source: string) => void, apply: () => void) {
  return EditorState.create({ doc: source, extensions: [
    Prec.highest(keymap.of([{ key: 'Mod-Enter', run: () => { apply(); return true; } }])),
    basicSetup, niminal, theme, colours, keymap.of([{ key: 'Tab', run: acceptCompletion }, indentWithTab]),
    lsp.plugin(uri, 'niminal'), semanticHighlighting,
    EditorView.updateListener.of(update => { if (update.docChanged) changed(update.state.doc.toString()); }),
  ] });
}
