import { Clip, Lane, LaneEvent, Snapshot, isSounding, phaseOf, windowOf } from './live';

type ActionHandler = (action: string, target?: string, change?: string) => void;

type NoteElement = {
  element: HTMLElement;
  start: number;
  dur: number;
};

type Row = {
  clip: Clip;
  element: HTMLElement;
  playhead: HTMLElement;
  notes: NoteElement[];
  parameters: NoteElement[];
  active: Set<HTMLElement>;
};

function element<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  return node;
}

function text(tag: keyof HTMLElementTagNameMap, className: string, value: string) {
  const node = element(tag, className);
  node.textContent = value;
  return node;
}

function button(label: string, title: string, action: string, extra?: Record<string, string>) {
  const node = element('button', 'clip-button');
  node.textContent = label;
  node.title = title;
  node.dataset.action = action;
  for (const [key, value] of Object.entries(extra ?? {})) node.dataset[key] = value;
  return node;
}

function bar(lane: Lane, event: LaneEvent, window: number, notes: boolean) {
  const node = element('div', notes ? 'event' : 'param-event');
  node.style.left = `${(event.start / window) * 100}%`;
  node.style.width = `${Math.max((event.dur / window) * 100, notes ? 1.5 : 0.35)}%`;
  const controls = [
    event.gain != null ? `gain ${event.gain}db` : '',
    event.pan != null ? `pan ${event.pan}deg` : '',
  ]
    .filter(Boolean)
    .join(' · ');
  node.title = `${lane.name}: ${event.value}${controls ? ` · ${controls}` : ''} · ${event.dur.toFixed(3)} cycles`;
  if (notes) node.textContent = event.value;
  return node;
}

/** The clip lanes, with a playhead and the sounding notes lit as they play. */
export class LaneView {
  private rows = new Map<string, Row>();
  private snapshot: Snapshot | null = null;
  private updatedAt = 0;
  private signature = '';

  constructor(
    private root: HTMLElement,
    private onAction: ActionHandler,
  ) {
    const frame = () => {
      this.paint();
      requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
  }

  update(snapshot: Snapshot) {
    this.snapshot = snapshot;
    this.updatedAt = performance.now();
    const signature = JSON.stringify(
      snapshot.clips.map(clip => [
        clip.name,
        clip.instrument,
        clip.running,
        clip.muted,
        clip.soloed,
        clip.edits,
        clip.length,
        clip.endless,
        clip.lanes.map(lane => [
          lane.name,
          lane.truncated,
          lane.events.map(event => [event.start, event.dur, event.value, event.gain, event.pan]),
        ]),
      ]),
    );
    if (signature !== this.signature) {
      this.signature = signature;
      this.rebuild(snapshot);
    }
    for (const clip of snapshot.clips) {
      const row = this.rows.get(clip.name);
      if (row) row.clip = clip;
    }
  }

  private rebuild(snapshot: Snapshot) {
    this.rows.clear();
    this.root.replaceChildren();
    if (!snapshot.connected) {
      const empty = element('div', 'stage-empty');
      empty.innerHTML =
        '<strong>No engine yet.</strong><span>Start niminal server, or let the stage start it, then evaluate the piece.</span>';
      this.root.append(empty);
      return;
    }
    if (snapshot.clips.length === 0) {
      const empty = element('div', 'stage-empty');
      empty.innerHTML =
        '<strong>The stage is empty.</strong><span>Evaluate a piece with clips to see its lanes here.</span>';
      this.root.append(empty);
      return;
    }
    for (const clip of snapshot.clips) this.root.append(this.row(clip));
  }

  private row(clip: Clip) {
    const article = element('article', 'clip');
    article.dataset.clip = clip.name;
    article.classList.toggle('running', clip.running);
    article.classList.toggle('muted', clip.muted);
    article.classList.toggle('soloed', clip.soloed);
    article.classList.toggle('idle', !clip.running);

    const head = element('header', 'clip-head');
    const state = element('span', 'clip-state');
    state.title = clip.running ? 'Playing' : 'Not playing';
    head.append(state);
    head.append(text('span', 'clip-name', clip.name));
    if (clip.instrument && clip.instrument !== clip.name) {
      head.append(text('span', 'clip-instrument', clip.instrument));
    }
    if (clip.running) {
      const loop = clip.endless
        ? 'endless'
        : `${clip.length.toFixed(clip.length % 1 === 0 ? 0 : 2)} bar${clip.length === 1 ? '' : 's'}`;
      head.append(text('span', 'clip-loop', loop));
    }

    const edits = element('span', 'clip-edits');
    for (const edit of clip.edits) {
      const chip = text('span', 'chip', edit.startsWith('reverse') ? `↺ ${edit}` : edit);
      chip.title = `Live change: ${edit}`;
      edits.append(chip);
    }
    head.append(edits);

    head.append(element('span', 'clip-spacer'));

    const controls = element('span', 'clip-controls');
    const running = button(
      clip.running ? '■' : '▶',
      clip.running ? `Stop ${clip.name}` : `Start ${clip.name}`,
      clip.running ? 'stop' : 'start',
    );
    running.classList.add('play');
    controls.append(running);
    const mute = button(clip.muted ? 'M' : 'M', clip.muted ? 'Unmute this clip' : 'Mute this clip', clip.muted ? 'unmute' : 'mute');
    mute.classList.toggle('on', clip.muted);
    controls.append(mute);
    const solo = button(clip.soloed ? 'S' : 'S', clip.soloed ? 'Unsolo this clip' : 'Solo this clip', clip.soloed ? 'unsolo' : 'solo');
    solo.classList.toggle('on', clip.soloed);
    controls.append(solo);
    controls.append(button('↺', 'Play this clip backwards', 'edit', { change: 'reverse' }));
    controls.append(button('∅', 'Thin this clip by 25%', 'edit', { change: 'thin(25%)' }));
    controls.append(button('↩', 'Take back the last change', 'edit', { change: 'undo' }));
    head.append(controls);
    article.append(head);

    const lanes = element('div', 'clip-lanes');
    const window = windowOf(clip);
    const noteLane = clip.lanes.find(lane => lane.name === 'notes') ?? clip.lanes[0];
    if (!noteLane) {
      const empty = text('div', 'clip-empty', clip.running ? 'playing from keys' : 'no lanes');
      lanes.append(empty);
    } else {
      lanes.append(this.lane(noteLane, window, true, clip));
      for (const lane of clip.lanes) {
        if (lane === noteLane) continue;
        lanes.append(this.lane(lane, window, false, clip));
      }
    }
    article.append(lanes);

    const playhead = element('div', 'playhead');
    playhead.append(element('span', 'playhead-dot'));
    gates(article).append(playhead);

    const notes: NoteElement[] = [];
    const parameters: NoteElement[] = [];
    for (const node of article.querySelectorAll<HTMLElement>('.event')) {
      notes.push({
        element: node,
        start: Number(node.dataset.start),
        dur: Number(node.dataset.dur),
      });
    }
    for (const node of article.querySelectorAll<HTMLElement>('.param-event')) {
      parameters.push({
        element: node,
        start: Number(node.dataset.start),
        dur: Number(node.dataset.dur),
      });
    }

    article.querySelectorAll<HTMLElement>('button[data-action]').forEach(node => {
      node.addEventListener('click', event => {
        event.stopPropagation();
        const action = node.dataset.action ?? '';
        this.onAction(action, clip.name, node.dataset.change);
      });
    });

    const row: Row = { clip, element: article, playhead, notes, parameters, active: new Set() };
    this.rows.set(clip.name, row);
    return article;
  }

  private lane(lane: Lane, window: number, notes: boolean, clip: Clip) {
    const wrap = element('div', notes ? 'lane notes-lane' : 'lane param-lane');
    wrap.append(text('span', 'lane-name', lane.name));
    const track = element('div', 'lane-track');
    const cycles = Math.max(1, window);
    track.style.backgroundSize = `${100 / cycles}% 100%`;
    for (const event of lane.events) {
      const node = bar(lane, event, window, notes);
      node.dataset.start = String(event.start);
      node.dataset.dur = String(event.dur);
      track.append(node);
    }
    if (lane.truncated) {
      const more = text('span', 'lane-more', '…');
      more.title = 'This lane has more events than the stage draws';
      track.append(more);
    }
    wrap.append(track);
    if (notes && !clip.lanes.some(item => item.name === 'notes') && clip.running) {
      wrap.title = 'This clip is played from a keyboard';
    }
    return wrap;
  }

  /** Move the playheads and light the notes that are sounding. */
  private paint() {
    const snapshot = this.snapshot;
    if (!snapshot?.transport) return;
    const bpm = snapshot.transport.bpm;
    const beats = snapshot.beats_per_bar || 4;
    const elapsed = (performance.now() - this.updatedAt) / 1000;
    for (const row of this.rows.values()) {
      const clip = row.clip;
      if (!clip.running || clip.cycles == null) {
        row.playhead.style.opacity = '0';
        for (const node of row.active) node.classList.remove('active');
        row.active.clear();
        continue;
      }
      const cycles = clip.cycles + (elapsed * bpm) / 60 / beats;
      const phase = phaseOf(clip, cycles);
      const window = windowOf(clip);
      row.playhead.style.opacity = '1';
      row.playhead.style.left = `${(phase / window) * 100}%`;
      const active = new Set<HTMLElement>();
      for (const note of row.notes) {
        if (isSounding(clip, note, cycles)) active.add(note.element);
      }
      for (const note of row.parameters) {
        if (isSounding(clip, note, cycles)) active.add(note.element);
      }
      for (const node of row.active) if (!active.has(node)) node.classList.remove('active');
      for (const node of active) {
        if (!row.active.has(node)) node.classList.add('active');
      }
      row.active = active;
    }
  }
}

function gates(article: HTMLElement) {
  let gate = article.querySelector<HTMLElement>('.clip-gates');
  if (!gate) {
    gate = element('div', 'clip-gates');
    article.append(gate);
  }
  return gate;
}
