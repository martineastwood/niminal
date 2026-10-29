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
  /** The loudest sample on each output channel since the last snapshot. */
  master: number[];
  /** The same for each clip and each bus, by name. */
  levels: { nodes: Record<string, number>; buses: Record<string, number> };
  /** The engine's running counts of things that make audio glitch; null when it does not play in real time. */
  health: {
    late_events: number;
    starved_frames: number;
    deadline_misses: number;
    capacity_drops: number;
    max_render_micros: number;
  } | null;
  /** Every control, with its range and what drives it. */
  /** Controls that took a MIDI control change since the last snapshot. */
  learned: { control: string; source: string }[];
  midi: { inputs: string[]; last: string | null };
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
  async connect(port: number, token: string) {
    await invoke<void>('connect', { port, token: token || null });
    // a missing MIDI device must not stop the stage
    await this.openMidi('').catch(() => []);
  },
  /** Listen to the MIDI inputs whose names contain `filter` (all of them when empty); returns their names. */
  openMidi(filter: string) {
    return invoke<string[]>('midi_open', { filter: filter || null });
  },
  /** The names of the MIDI inputs and outputs there are. */
  midiPorts() {
    return invoke<{ inputs: string[]; outputs: string[] }>('midi_ports');
  },
  /** Send MIDI clock to the output called `name` (made if there is none), or stop with `null`. */
  clockOut(name: string | null) {
    return invoke<void>('clock_out', { name });
  },
  /** Follow the MIDI clock of the input called `name`, or stop with `null`. */
  clockIn(name: string | null) {
    return invoke<void>('clock_in', { name });
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
  openPath(path: string) {
    return invoke<OpenedFile>('open_path', { path });
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
  /** Render engine samples `from` to `to` to a WAV file the user picks, from the engine's own session log. */
  renderTake(from: number, to: number) {
    return invoke<string | null>('render_take', { from, to });
  },
  /** Save what was evaluated in samples `from` to `to` as a script the user picks the place of. */
  saveTranscript(from: number, to: number) {
    return invoke<string | null>('save_transcript', { from, to });
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

/** How many beats until sample `landsAt`, with `elapsed` seconds gone since the snapshot was taken. */
export function beatsUntil(snapshot: Snapshot, landsAt: number, elapsed = 0) {
  const t = snapshot.transport;
  if (!t || !snapshot.sample_rate) return 0;
  const now = t.sample + elapsed * snapshot.sample_rate;
  return Math.max(0, ((landsAt - now) / snapshot.sample_rate) * (t.bpm / 60));
}

export type Queued = { kind: 'start' | 'stop'; landsAt: number };

/**
 * The start or stop each clip and scene has waiting, from the changes that have not landed yet. A scene's
 * start is also a start for each of its clips that is not playing, and a stop for each playing clip of
 * another scene that it does not use, because that is what lands.
 */
export function queuedStates(snapshot: Snapshot) {
  const queued = new Map<string, Queued>();
  const set = (name: string, kind: Queued['kind'], landsAt: number) => {
    if (!queued.has(name)) queued.set(name, { kind, landsAt });
  };
  const byTime = [...snapshot.pending].sort((a, b) => a.lands_at - b.lands_at);
  for (const change of byTime) {
    for (const text of change.changes) {
      const match = /^\s*([A-Za-z_]\w*)\.(start|stop)\b/.exec(text);
      if (!match) continue;
      const [, name, kind] = match;
      const scene = snapshot.scenes.find(s => s.name === name);
      set(name, kind as Queued['kind'], change.lands_at);
      if (!scene) continue;
      const running = (clip: string) => snapshot.clips.some(c => c.name === clip && c.running);
      if (kind === 'stop') {
        for (const clip of scene.clips) if (running(clip)) set(clip, 'stop', change.lands_at);
        continue;
      }
      for (const clip of scene.clips) if (!running(clip)) set(clip, 'start', change.lands_at);
      for (const other of snapshot.scenes) {
        if (other === scene) continue;
        for (const clip of other.clips) if (!scene.clips.includes(clip) && running(clip)) set(clip, 'stop', change.lands_at);
      }
    }
  }
  return queued;
}
