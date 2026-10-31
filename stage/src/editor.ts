import { basicSetup } from 'codemirror';
import { EditorState, Prec } from '@codemirror/state';
import { EditorView, keymap } from '@codemirror/view';
import { acceptCompletion } from '@codemirror/autocomplete';
import { indentWithTab } from '@codemirror/commands';
import { isTauri } from '@tauri-apps/api/core';
import DOMPurify from 'dompurify';
import { colours, niminal } from '../../editors/codemirror/language';
import { connectLanguageServer as connect, editorTheme, languageClient } from '../../editors/codemirror/lsp';
import { stage } from './live';
import { highlightExtensions } from './highlight';
import { scrubbing } from './scrub';
import { semanticHighlighting } from '../../editors/codemirror/semantic';

export const lsp = languageClient(html => DOMPurify.sanitize(html));

export async function connectLanguageServer(status: (message: string) => void) {
  if (!isTauri()) {
    status('Browser preview · open the desktop app for the language server');
    return;
  }
  await connect(lsp, message => stage.lspMessage(message) as Promise<Record<string, unknown>[]>, status);
}

const theme = editorTheme('calc(14px * var(--z))');

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
      scrubbing(() => apply('chunk')),
      basicSetup,
      EditorView.lineWrapping,
      niminal,
      theme,
      colours,
      highlightExtensions,
      semanticHighlighting,
      // Tab takes the suggestion when the list is open, and indents otherwise
      keymap.of([{ key: 'Tab', run: acceptCompletion }, indentWithTab]),
      lsp.plugin(uri, 'niminal'),
      EditorView.updateListener.of(update => {
        if (update.docChanged) changed?.();
      }),
    ],
  });
}
