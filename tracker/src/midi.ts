import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export type MidiMessage = { kind: 'on'; channel: number; note: number; velocity: number }
  | { kind: 'off'; channel: number; note: number }
  | { kind: 'panic'; channel: number }
  | { kind: 'reset' };
interface Port { id: string; name: string }

/** One explicitly chosen native input; disabling, unplugging, or changing channels releases previews. */
export async function mountMidi(parent: HTMLElement, receive: (message: MidiMessage) => void, feedback: (text: string) => void) {
  parent.innerHTML = `<label>MIDI <select aria-label="MIDI input"><option value="">Off</option></select></label><label>Ch <select aria-label="MIDI channel"><option value="">All</option>${Array.from({ length: 16 }, (_, i) => `<option value="${i}">${i+1}</option>`).join('')}</select></label><button aria-label="Refresh MIDI inputs" title="Refresh MIDI inputs">↻</button>`;
  const input = parent.querySelector<HTMLSelectElement>('[aria-label="MIDI input"]')!;
  const channel = parent.querySelector<HTMLSelectElement>('[aria-label="MIDI channel"]')!;
  const refreshButton = parent.querySelector<HTMLButtonElement>('button')!;
  if (!isTauri()) {
    input.disabled = channel.disabled = refreshButton.disabled = true;
    parent.title = 'Open the desktop app to use a MIDI keyboard';
    return () => {};
  }
  let selected = '', busy = false, disposed = false;
  const reset = () => receive({ kind: 'reset' });
  const unlisten = await listen<{ port: string; message: Exclude<MidiMessage, { kind: 'reset' }> }>('midi', event => {
    if (disposed || event.payload.port !== selected) return;
    const message = event.payload.message;
    if (message.kind === 'on' && !document.hasFocus()) return;
    if (channel.value !== '' && message.channel !== Number(channel.value)) return;
    receive(message);
  });
  const refresh = async () => {
    if (busy || disposed) return;
    busy = true;
    try {
      const ports = await invoke<Port[]>('midi_ports');
      if (disposed) return;
      if (selected && !ports.some(p => p.id === selected)) {
        selected = ''; reset(); await invoke('midi_connect', { port: null });
        feedback('MIDI input disconnected. Reconnect it and select it again.');
      }
      input.replaceChildren(new Option('Off', ''), ...ports.map(p => new Option(p.name, p.id)));
      input.value = selected;
      input.title = ports.length ? 'Choose a MIDI keyboard or input port' : 'No MIDI inputs found; connect a keyboard and refresh';
    } catch (error) { reset(); feedback(`MIDI: ${error}`); }
    finally { busy = false; }
  };
  input.onchange = async () => {
    if (busy) { input.value = selected; return; }
    const next = input.value;
    busy = true; input.disabled = true; selected = ''; reset();
    try { await invoke('midi_connect', { port: next || null }); selected = next; }
    catch (error) { input.value = ''; feedback(`MIDI: ${error}`); }
    finally { busy = false; input.disabled = false; }
  };
  channel.onchange = reset;
  refreshButton.onclick = () => { void refresh(); };
  window.addEventListener('blur', reset);
  await refresh();
  const timer = window.setInterval(() => { if (selected) void refresh(); }, 2000);
  return () => { disposed = true; window.clearInterval(timer); window.removeEventListener('blur', reset); unlisten(); reset(); void invoke('midi_connect', { port: null }).catch(() => {}); };
}
