import { expect, it } from 'vitest';
import { languageTransport } from './transport';

it('orders document updates ahead of requests even when the backend is asynchronous', async () => {
  const calls: string[] = [], replies: string[] = [];
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const transport = languageTransport(async message => {
    calls.push(String(message.method));
    if (message.method === 'change') await gate;
    return [{ id: message.id ?? null }];
  }, error => { throw error; });
  transport.subscribe(reply => replies.push(reply));
  transport.send(JSON.stringify({ method: 'change' }));
  transport.send(JSON.stringify({ method: 'hover', id: 2 }));
  await Promise.resolve();
  expect(calls).toEqual(['change']);
  release();
  await new Promise(resolve => setTimeout(resolve, 0));
  expect(calls).toEqual(['change', 'hover']);
  expect(replies.map(reply => JSON.parse(reply).id)).toEqual([null, 2]);
});

it('returns a failed request to the client and keeps processing after a backend error', async () => {
  const errors: unknown[] = [], replies: string[] = [];
  const transport = languageTransport(async message => {
    if (message.id === 1) throw new Error('compile worker unavailable');
    return [{ id: message.id, result: 'ready' }];
  }, error => errors.push(error));
  transport.subscribe(reply => replies.push(reply));
  transport.send('{"id":1}'); transport.send('{"id":2}');
  await new Promise(resolve => setTimeout(resolve, 0));
  expect(errors).toHaveLength(1);
  expect(JSON.parse(replies[0]).error.message).toContain('compile worker unavailable');
  expect(JSON.parse(replies[1]).result).toBe('ready');
});
