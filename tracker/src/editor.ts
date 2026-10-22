import { basicSetup } from 'codemirror';
import { EditorState } from '@codemirror/state';
import { EditorView, keymap } from '@codemirror/view';
import { indentWithTab } from '@codemirror/commands';
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { tags } from '@lezer/highlight';
import { LSPClient, serverDiagnostics, serverCompletion, hoverTooltips, signatureHelp, formatKeymap } from '@codemirror/lsp-client';
import { invoke, isTauri } from '@tauri-apps/api/core';
import DOMPurify from 'dompurify';
import { niminal } from './language';
import { languageTransport } from './transport';

export const lsp = new LSPClient({
  extensions: [serverDiagnostics(), serverCompletion(), hoverTooltips(), signatureHelp(), keymap.of(formatKeymap)],
  sanitizeHTML: html => DOMPurify.sanitize(html),
});

export async function connectLanguageServer(status: (message: string) => void) {
  if (!isTauri()) { status('Browser preview · open the desktop app for LSP'); return; }
  lsp.connect(languageTransport(message => invoke('lsp_message', { message }), error => status(`Language server error: ${error}`)));
  try { await lsp.initializing; status('niminal LSP ready'); }
  catch (error) { status(`Language server unavailable: ${error}`); }
}

const theme = EditorView.theme({
  '&': { height: '100%', backgroundColor: '#151b22', color: '#d4dde7', fontSize: '13px' },
  '.cm-scroller': { overflow: 'auto', fontFamily: '"SFMono-Regular", Consolas, monospace' },
  '.cm-content': { padding: '16px 0', caretColor: '#e9b66e' },
  '.cm-gutters': { backgroundColor: '#151b22', color: '#657486', border: 'none' },
  '.cm-activeLine, .cm-activeLineGutter': { backgroundColor: '#202a35' },
  '&.cm-focused .cm-selectionBackground, .cm-selectionBackground': { backgroundColor: '#384b60' },
  '.cm-tooltip': { backgroundColor: '#25313e', color: '#e1e7ed', border: '1px solid #4b5e73', maxWidth: '550px' },
  '.cm-tooltip pre': { whiteSpace: 'pre-wrap' },
}, { dark: true });
const colours = syntaxHighlighting(HighlightStyle.define([
  { tag: tags.keyword, color: '#c6a1e8' },
  { tag: tags.comment, color: '#819184' },
  { tag: tags.string, color: '#a5c98d' },
  { tag: [tags.number, tags.atom], color: '#e9b66e' },
  { tag: tags.typeName, color: '#74c7c3' },
  { tag: tags.function(tags.variableName), color: '#8cbce7' },
]));

export function editorState(uri: string, source: string, changed: (source: string) => void) {
  return EditorState.create({ doc: source, extensions: [
    basicSetup, niminal, theme, colours, keymap.of([indentWithTab]),
    lsp.plugin(uri, 'niminal'),
    EditorView.updateListener.of(update => { if (update.docChanged) changed(update.state.doc.toString()); }),
  ] });
}
