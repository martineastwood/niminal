// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { mountMidi, type MidiMessage } from './midi';

const native = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn(), listen: vi.fn(), unlisten: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: native.invoke, isTauri: native.isTauri }));
vi.mock('@tauri-apps/api/event', () => ({ listen: native.listen }));
let handler: (event: { payload: { port: string; message: Exclude<MidiMessage, { kind: 'reset' }> } }) => void;
let dispose: (() => void) | undefined;
let ports = [{ id: 'keys', name: 'Keyboard' }];
beforeEach(() => {
  vi.clearAllMocks(); ports = [{ id: 'keys', name: 'Keyboard' }];
  native.isTauri.mockReturnValue(true);
  native.invoke.mockImplementation(async (command: string) => command === 'midi_ports' ? ports : undefined);
  native.listen.mockImplementation(async (_: string, callback: typeof handler) => { handler = callback; return native.unlisten; });
  vi.spyOn(document, 'hasFocus').mockReturnValue(true);
  vi.stubGlobal('Option', function (text: string, value: string) { const o = document.createElement('option'); o.textContent = text; o.value = value; return o; });
});
afterEach(() => { dispose?.(); dispose = undefined; document.body.replaceChildren(); vi.restoreAllMocks(); });
async function setup() {
  const parent = document.createElement('div'); document.body.append(parent);
  const receive = vi.fn(), feedback = vi.fn(); dispose = await mountMidi(parent, receive, feedback);
  const input = parent.querySelector<HTMLSelectElement>('[aria-label="MIDI input"]')!, channel = parent.querySelector<HTMLSelectElement>('[aria-label="MIDI channel"]')!;
  const select = async (value: string) => { input.value = value; await input.onchange!.call(input, new Event('change')); };
  return { parent, receive, feedback, input, channel, select };
}
it('opens only the selected native input and filters channels, releasing previews on changes', async () => {
  const { receive, input, channel, select } = await setup();
  expect(input.options[1].textContent).toBe('Keyboard');
  const note: MidiMessage = { kind: 'on', channel: 1, note: 60, velocity: 100 };
  handler({ payload: { port: 'keys', message: note } }); expect(receive).not.toHaveBeenCalled();
  await select('keys'); expect(native.invoke).toHaveBeenCalledWith('midi_connect', { port: 'keys' });
  handler({ payload: { port: 'keys', message: note } }); expect(receive).toHaveBeenLastCalledWith(note);
  channel.value = '0'; channel.dispatchEvent(new Event('change')); expect(receive).toHaveBeenLastCalledWith({ kind: 'reset' });
  receive.mockClear(); handler({ payload: { port: 'keys', message: note } }); expect(receive).not.toHaveBeenCalled();
  handler({ payload: { port: 'other', message: { ...note, channel: 0 } } }); expect(receive).not.toHaveBeenCalled();
  handler({ payload: { port: 'keys', message: { ...note, channel: 0 } } }); expect(receive).toHaveBeenCalledTimes(1);
  window.dispatchEvent(new Event('blur')); expect(receive).toHaveBeenLastCalledWith({ kind: 'reset' });
  await select(''); expect(native.invoke).toHaveBeenCalledWith('midi_connect', { port: null });
});
it('ignores background note-ons but still releases keys, and detects an unplugged input', async () => {
  const { parent, receive, feedback, input, select } = await setup(); await select('keys'); receive.mockClear();
  vi.mocked(document.hasFocus).mockReturnValue(false);
  handler({ payload: { port: 'keys', message: { kind: 'on', channel: 0, note: 60, velocity: 100 } } }); expect(receive).not.toHaveBeenCalled();
  handler({ payload: { port: 'keys', message: { kind: 'off', channel: 0, note: 60 } } }); expect(receive).toHaveBeenLastCalledWith({ kind: 'off', channel: 0, note: 60 });
  ports = []; parent.querySelector('button')!.click();
  await vi.waitFor(() => expect(input.value).toBe(''));
  expect(receive).toHaveBeenLastCalledWith({ kind: 'reset' }); expect(feedback).toHaveBeenCalledWith(expect.stringContaining('disconnected'));
  expect(native.invoke).toHaveBeenCalledWith('midi_connect', { port: null });
});
it('shows connection failures and disables MIDI controls in browser previews', async () => {
  const { feedback, input, select } = await setup();
  native.invoke.mockRejectedValueOnce(new Error('device unavailable')); await select('keys');
  expect(input.value).toBe(''); expect(input.disabled).toBe(false); expect(feedback).toHaveBeenCalledWith(expect.stringContaining('device unavailable'));
  dispose!(); dispose = undefined;
  native.isTauri.mockReturnValue(false); const preview = await setup();
  expect(preview.input.disabled).toBe(true); expect(preview.channel.disabled).toBe(true);
});
