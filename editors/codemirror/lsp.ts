import { EditorView, keymap } from '@codemirror/view';
import {
  LSPClient,
  formatKeymap,
  hoverTooltips,
  serverCompletion,
  serverDiagnostics,
  signatureHelp,
  type Transport,
} from '@codemirror/lsp-client';

type Message = Record<string, unknown>;
export type Exchange = (message: Message) => Promise<Message[]>;

// LSP notifications and requests must reach the compiler in document order.
// Tauri invocations alone do not guarantee that ordering.
export function languageTransport(exchange: Exchange, onError: (error: unknown) => void): Transport {
  const handlers = new Set<(message: string) => void>();
  let pending = Promise.resolve();
  return {
    send(raw) {
      const message: Message = JSON.parse(raw);
      pending = pending.then(async () => {
        try {
          for (const reply of await exchange(message)) {
            for (const handler of handlers) handler(JSON.stringify(reply));
          }
        } catch (error) {
          onError(error);
          if ('id' in message) {
            const reply = JSON.stringify({ jsonrpc: '2.0', id: message.id, error: { code: -32603, message: String(error) } });
            for (const handler of handlers) handler(reply);
          }
        }
      });
    },
    subscribe(handler) { handlers.add(handler); },
    unsubscribe(handler) { handlers.delete(handler); },
  };
}

/** The language server's client: diagnostics, completion, hovers, signatures and formatting. `sanitizeHTML` cleans its docs. */
export function languageClient(sanitizeHTML: (html: string) => string) {
  return new LSPClient({
    extensions: [serverDiagnostics(), serverCompletion(), hoverTooltips(), signatureHelp(), keymap.of(formatKeymap)],
    sanitizeHTML,
  });
}

/** Connect `client` to the language server through `exchange`, saying how it went. */
export async function connectLanguageServer(client: LSPClient, exchange: Exchange, status: (message: string) => void) {
  client.connect(languageTransport(exchange, error => status(`Language server error: ${error}`)));
  try {
    await client.initializing;
    status('niminal LSP ready');
  } catch (error) {
    status(`Language server unavailable: ${error}`);
  }
}

/** The dark editor theme both apps use, with the code at `fontSize`. */
export function editorTheme(fontSize: string) {
  return EditorView.theme(
    {
      '&': { height: '100%', backgroundColor: '#14161a', color: '#d3d7de', fontSize },
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
}
