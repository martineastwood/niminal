import { basicSetup } from 'codemirror';
import { EditorState, Prec } from '@codemirror/state';
import { EditorView, keymap } from '@codemirror/view';
import { indentWithTab } from '@codemirror/commands';
import {
  LSPClient,
  serverDiagnostics,
  serverCompletion,
  hoverTooltips,
  signatureHelp,
  formatKeymap,
} from '@codemirror/lsp-client';
import { isTauri } from '@tauri-apps/api/core';
import DOMPurify from 'dompurify';
import { colours, niminal } from '../../editors/codemirror/language';
import { languageTransport } from './transport';
import { stage } from './live';
import { highlightExtensions } from './highlight';

export const lsp = new LSPClient({
  extensions: [
    serverDiagnostics(),
    serverCompletion(),
    hoverTooltips(),
    signatureHelp(),
    keymap.of(formatKeymap),
  ],
  sanitizeHTML: html => DOMPurify.sanitize(html),
});

export async function connectLanguageServer(status: (message: string) => void) {
  if (!isTauri()) {
    status('Browser preview · open the desktop app for the language server');
    return;
  }
  lsp.connect(
    languageTransport(
      message => stage.lspMessage(message) as Promise<Record<string, unknown>[]>,
      error => status(`Language server error: ${error}`),
    ),
  );
  try {
    await lsp.initializing;
    status('niminal LSP ready');
  } catch (error) {
    status(`Language server unavailable: ${error}`);
  }
}

const theme = EditorView.theme(
  {
    '&': { height: '100%', backgroundColor: '#14161a', color: '#d3d7de', fontSize: 'calc(14px * var(--z))' },
    '.cm-scroller': { overflow: 'auto', fontFamily: '"SF Mono", "SFMono-Regular", Consolas, monospace' },
    '.cm-content': { padding: '6px 0', caretColor: '#e9b66e' },
    '.cm-gutters': { backgroundColor: '#14161a', color: '#5f6670', border: 'none' },
    // translucent, so a selection on the cursor's line still shows through it
    '.cm-activeLine, .cm-activeLineGutter': { backgroundColor: 'rgba(255, 255, 255, 0.045)' },
    '.cm-selectionBackground': { backgroundColor: 'rgba(140, 188, 231, 0.22)' },
    '&.cm-focused .cm-selectionBackground, &.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground': {
      backgroundColor: 'rgba(140, 188, 231, 0.45)',
    },
    '.cm-selectionMatch': { backgroundColor: 'rgba(233, 182, 110, 0.25)', outline: '1px solid rgba(233, 182, 110, 0.4)' },
    '.cm-tooltip': { backgroundColor: '#23272d', color: '#e1e7ed', border: '1px solid #3d434b', maxWidth: '560px' },
    '.cm-tooltip pre': { whiteSpace: 'pre-wrap' },
    '.cm-diagnostic-error': { borderLeft: '2px solid #d86a6a' },
  },
  { dark: true },
);

/** What an evaluation sends: the selection or the block around the cursor, or the whole file. */
export type Scope = 'chunk' | 'file';

export function niminalEditor(source: string, uri: string, apply: (scope: Scope) => void, changed?: () => void) {
  return EditorState.create({
    doc: source,
    extensions: [
      Prec.highest(
        keymap.of([
          {
            key: 'Mod-Enter',
            run: () => {
              apply('chunk');
              return true;
            },
          },
          {
            key: 'Mod-Shift-Enter',
            run: () => {
              apply('file');
              return true;
            },
          },
        ]),
      ),
      basicSetup,
      niminal,
      theme,
      colours,
      highlightExtensions,
      keymap.of([indentWithTab]),
      lsp.plugin(uri, 'niminal'),
      EditorView.updateListener.of(update => {
        if (update.docChanged) changed?.();
      }),
    ],
  });
}
