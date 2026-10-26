import { invoke, isTauri } from '@tauri-apps/api/core';

export type LaneEvent = {
  start: number;
  dur: number;
  value: string;
  gain: number | null;
  pan: number | null;
  /** Byte range of the note in the evaluated source. */
  src: [number, number] | null;
  /** Which evaluation sent that text: the number the stage gave it. */
  eval: number | null;
};

export type Lane = {
  name: string;
  events: LaneEvent[];
  truncated: boolean;
};

export type Clip = {
  name: string;
  instrument: string;
  running: boolean;
  muted: boolean;
  soloed: boolean;
  edits: string[];
  length: number;
  looped: boolean;
  /** How many times it plays before it is finished, if it is not for ever. */
  plays?: number | null;
  endless: boolean;
  cycles: number | null;
  lanes: Lane[];
};

export type Scene = {
  name: string;
  clips: string[];
  running: number;
};

export type Pending = {
  id: number;
  lands_at: number;
  in_samples: number;
  position: { bar: number; beat: number };
  changes: string[];
};

export type Transport = {
  sample: number;
  seconds: number;
  bar: number;
  beat: number;
  bpm: number;
  voices: number;
  pending: number;
};

export type Snapshot = {
  connected: boolean;
  revision: number;
  transport: Transport | null;
  beats_per_bar: number;
  meter: [number, number];
  sample_rate: number;
  channels: number;
  clips: Clip[];
  scenes: Scene[];
  pending: Pending[];
  landed: { id: number; at: number; position: { bar: number; beat: number }; changes: string[] }[];
  notices: string[];
  error: string | null;
};

export type Accepted = {
  id: number | null;
  lands_at: number;
  position: { bar: number; beat: number };
  changes: string[];
};

export type Problem = {
  message: string;
  help: string | null;
  line: number | null;
  column: number | null;
  in_definition: string | null;
};

export type EvalOutcome = {
  accepted: Accepted | null;
  problems: Problem[];
};

export type OpenedFile = {
  path: string;
  dir: string;
  source: string;
};

export type OpenedProject = {
  dir: string;
  /** The file that plays the project, when it has a `main.nml`. */
  entry: string | null;
  files: OpenedFile[];
};

export type Action = {
  action: string;
  target?: string;
  change?: string;
};

const desktop = isTauri();

export const stage = {
  get desktop() {
    return desktop;
  },
  connect(port: number, token: string) {
    return invoke<void>('connect', { port, token: token || null });
  },
  disconnect() {
    return invoke<void>('disconnect');
  },
  snapshot() {
    return invoke<Snapshot>('snapshot');
  },
  async evaluate(source: string, quantize: string, dir: string | null, origin: number) {
    return invoke<EvalOutcome>('eval', { source, quantize: quantize || null, dir, origin });
  },
  perform(action: Action, quantize: string) {
    return invoke<EvalOutcome>('perform', {
      action: action.action,
      target: action.target ?? null,
      change: action.change ?? null,
      quantize: quantize || null,
    });
  },
  cancel(id: number | null) {
    return invoke<number>('cancel', { id });
  },
  openFile() {
    return invoke<OpenedFile | null>('open_file');
  },
  openProject() {
    return invoke<OpenedProject | null>('open_project');
  },
  saveFile(path: string, source: string) {
    return invoke<void>('save_file', { path, source });
  },
  saveFileAs(source: string) {
    return invoke<string | null>('save_file_as', { source });
  },
  lspMessage(message: unknown) {
    return invoke<unknown[]>('lsp_message', { message });
  },
};

/** How long a loop is in cycles: an endless clip is drawn as one cycle at a time. */
export function windowOf(clip: Clip) {
  return clip.endless ? 1 : Math.max(clip.length, 1e-6);
}

/** Where in the drawn window the clip is, at `cycles` since it started. */
export function phaseOf(clip: Clip, cycles: number) {
  const window = windowOf(clip);
  if (!clip.endless) {
    const phase = cycles % window;
    return phase < 0 ? phase + window : phase;
  }
  return cycles % window;
}

/**
 * How many cycles ago `event` began, at `cycles` since the clip started, or null if it has not begun yet.
 * A clip that loops starts its notes over each pass, so its lane is read as a loop. An endless clip's
 * lane holds only the cycle it was read in (the next may differ: `every`, `<a b>`, a reverse), so it is
 * not wrapped: past that cycle's end its notes fade out and the next cycle's wait for the next reading.
 */
export function sinceOnset(clip: Clip, event: { start: number }, cycles: number) {
  if (clip.endless) {
    const age = cycles - Math.floor(clip.cycles ?? 0) - event.start;
    return age >= 0 ? age : null;
  }
  const window = windowOf(clip);
  return (((phaseOf(clip, cycles) - event.start) % window) + window) % window;
}

/** A note is lit while it sounds; a zero-length note gets a short flash so it can be seen. */
export function isSounding(clip: Clip, event: { start: number; dur: number }, cycles: number) {
  const age = sinceOnset(clip, event, cycles);
  const flash = Math.max(event.dur, windowOf(clip) * 0.06);
  return age !== null && age < flash;
}

/** A stable description of everything the lane view draws, so it is only rebuilt when that changes. */
export function laneSignature(clips: Clip[]) {
  return JSON.stringify(
    clips.map(clip => [
      clip.name,
      clip.instrument,
      clip.running,
      clip.muted,
      clip.soloed,
      clip.edits,
      clip.length,
      clip.endless,
      clip.lanes.map(lane => [lane.name, lane.events.map(event => [event.start, event.dur, event.value])]),
    ]),
  );
}
