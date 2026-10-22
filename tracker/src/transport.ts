import type { Transport } from '@codemirror/lsp-client';

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
