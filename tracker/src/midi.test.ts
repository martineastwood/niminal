// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { mountMidi, MidiPerformance, type MidiMessage } from './midi';

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


it('sustains released keys per channel and leaves physically pressed keys sounding on pedal-up', () => {
  const emit = vi.fn(), midi = new MidiPerformance(emit);
  const on = (channel: number, note: number) => midi.receive({ kind: 'on', channel, note, velocity: 100 });
  on(0, 60); on(0, 64); on(1, 60);
  midi.receive({ kind: 'sustain', channel: 0, down: true });
  midi.receive({ kind: 'sustain', channel: 1, down: true });
  emit.mockClear();
  midi.receive({ kind: 'off', channel: 0, note: 60 });
  midi.receive({ kind: 'off', channel: 1, note: 60 });
  expect(emit).not.toHaveBeenCalled();
  midi.receive({ kind: 'sustain', channel: 0, down: false });
  expect(emit.mock.calls.map(([m]) => m)).toEqual([{ kind: 'off', channel: 0, note: 60 }]);
  midi.receive({ kind: 'off', channel: 0, note: 64 });
  expect(emit).toHaveBeenLastCalledWith({ kind: 'off', channel: 0, note: 64 });
  midi.receive({ kind: 'sustain', channel: 1, down: false });
  expect(emit).toHaveBeenLastCalledWith({ kind: 'off', channel: 1, note: 60 });
});

it('re-presses a sustained pitch in order and does not release its replacement with the pedal', () => {
  const emit = vi.fn(), midi = new MidiPerformance(emit);
  const note: MidiMessage = { kind: 'on', channel: 2, note: 60, velocity: 80 };
  midi.receive(note); midi.receive({ kind: 'sustain', channel: 2, down: true });
  midi.receive({ kind: 'off', channel: 2, note: 60 }); emit.mockClear();
  midi.receive({ ...note, velocity: 110 });
  expect(emit.mock.calls.map(([m]) => m)).toEqual([{ kind: 'off', channel: 2, note: 60 }, { ...note, velocity: 110 }]);
  emit.mockClear(); midi.receive({ kind: 'sustain', channel: 2, down: false });
  expect(emit).not.toHaveBeenCalled();
  midi.receive({ kind: 'off', channel: 2, note: 60 });
  expect(emit).toHaveBeenCalledOnce();
});

it('panic clears only its channel and reset forgets all pedals and deferred releases', () => {
  const emit = vi.fn(), midi = new MidiPerformance(emit);
  for (const channel of [0, 1]) {
    midi.receive({ kind: 'sustain', channel, down: true });
    midi.receive({ kind: 'on', channel, note: 60, velocity: 100 });
    midi.receive({ kind: 'off', channel, note: 60 });
  }
  emit.mockClear(); midi.receive({ kind: 'panic', channel: 0 });
  midi.receive({ kind: 'sustain', channel: 0, down: false });
  expect(emit.mock.calls.map(([m]) => m)).toEqual([{ kind: 'panic', channel: 0 }]);
  midi.receive({ kind: 'sustain', channel: 1, down: false });
  expect(emit).toHaveBeenLastCalledWith({ kind: 'off', channel: 1, note: 60 });
  midi.receive({ kind: 'sustain', channel: 0, down: true });
  midi.receive({ kind: 'on', channel: 0, note: 60, velocity: 100 });
  midi.receive({ kind: 'off', channel: 0, note: 60 });
  midi.receive({ kind: 'reset' }); emit.mockClear();
  midi.receive({ kind: 'sustain', channel: 0, down: false }); expect(emit).not.toHaveBeenCalled();
  midi.receive({ kind: 'on', channel: 0, note: 62, velocity: 100 }); emit.mockClear();
  midi.receive({ kind: 'off', channel: 0, note: 62 });
  expect(emit).toHaveBeenCalledWith({ kind: 'off', channel: 0, note: 62 });
});

it('filters sustain with the selected channel and ignores background pedal-down', async () => {
  const { receive, channel, select } = await setup(); await select('keys'); receive.mockClear();
  const send = (ch: number, down: boolean) => handler({ payload: { port: 'keys', message: { kind: 'sustain', channel: ch, down } } });
  channel.value = '2'; send(1, true); expect(receive).not.toHaveBeenCalled();
  send(2, true); expect(receive).toHaveBeenLastCalledWith({ kind: 'sustain', channel: 2, down: true });
  receive.mockClear(); vi.mocked(document.hasFocus).mockReturnValue(false);
  send(2, true); expect(receive).not.toHaveBeenCalled();
  send(2, false); expect(receive).toHaveBeenLastCalledWith({ kind: 'sustain', channel: 2, down: false });
});


it('forwards CCs without releasing sustained notes and filters background control movement', async () => {
  const emit = vi.fn(), performance = new MidiPerformance(emit);
  performance.receive({ kind: 'sustain', channel: 0, down: true });
  performance.receive({ kind: 'on', channel: 0, note: 60, velocity: 100 });
  performance.receive({ kind: 'off', channel: 0, note: 60 }); emit.mockClear();
  const message: MidiMessage = { kind: 'control', channel: 0, controller: 7, value: 80 };
  performance.receive(message); expect(emit).toHaveBeenCalledOnce(); expect(emit).toHaveBeenLastCalledWith(message);
  performance.receive({ kind: 'sustain', channel: 0, down: false }); expect(emit).toHaveBeenLastCalledWith({ kind: 'off', channel: 0, note: 60 });
  const { receive, select } = await setup(); await select('keys'); receive.mockClear();
  vi.mocked(document.hasFocus).mockReturnValue(false);
  handler({ payload: { port: 'keys', message } }); expect(receive).not.toHaveBeenCalled();
});
