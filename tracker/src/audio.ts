import { invoke, isTauri } from '@tauri-apps/api/core';
import type { EffectParameters, Song } from './project';

export interface AudioStatus {
  connected: boolean; playing: boolean; row: number | null; row_phase: number | null; row_at_ms: number; pattern_id: string | null; sequence_position: number | null; sample_rate: number;
  skipped_rows: number; late_events: number; error: string | null;
  instruments: Record<string, string>; effects: Record<string, string>; effect_parameters: EffectParameters; instrument_parameters: EffectParameters;
}
type Command =
  | { op: 'connect'; port: number; token: string | null }
  | { op: 'disconnect' | 'stop' }
  | { op: 'apply'; id: string; source: string; kind: 'instr' | 'fx'; dir: string | null }
  | { op: 'play'; song: Song; mode: 'pattern' | 'song'; pattern_id: string; start_position: number }
  | { op: 'update'; song: Song }
  | { op: 'export'; song: Song; path: string; stems: boolean }
  | { op: 'scopes'; strips: boolean }
  | { op: 'midi_on'; key: number; instrument: string; note: string; track: number; velocity: number }
  | { op: 'midi_off'; key: number | null }
  | { op: 'audition'; instrument: string; note: string; track: number; ms?: number; commands?: string[] };

export let audioStatus: AudioStatus = {
  connected: false, playing: false, row: null, row_phase: null, row_at_ms: 0, pattern_id: null, sequence_position: null, sample_rate: 0,
  skipped_rows: 0, late_events: 0, error: null, instruments: {}, effects: {}, effect_parameters: {}, instrument_parameters: {},
};
let receive: (status: AudioStatus) => void = () => {};
let pending = Promise.resolve();
let polling = false;

function update(status: AudioStatus) { audioStatus = status; receive(status); }

export function observeAudio(callback: typeof receive) {
  receive = callback;
  receive(audioStatus);
  // This timer paints transport state only. Musical scheduling lives in Rust.
  let idle = 0;
  return window.setInterval(async () => {
    // Nothing moves while stopped, and commands report the status they cause, so look less often then.
    idle = audioStatus.playing ? 0 : idle + 1;
    if (!isTauri() || polling || idle > 4 && idle % 4 !== 0) return;
    polling = true;
    try { update(await invoke<AudioStatus>('audio_status')); }
    catch (error) { update({ ...audioStatus, error: String(error) }); }
    finally { polling = false; }
  }, 100);
}

export function audioCommand(command: Command): Promise<AudioStatus> {
  const result = pending.then(async () => {
    if (!isTauri()) throw new Error('Open the desktop app to connect to niminal');
    const status = await invoke<AudioStatus>('audio_command', { command });
    update(status);
    return status;
  });
  pending = result.then(() => {}, () => {});
  return result;
}
