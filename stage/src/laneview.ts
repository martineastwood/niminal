import { CLIP_COLOURS } from './highlight';
import { mod, shortcuts } from './shortcuts';
import { Bar } from './meters';
import { Clip, Lane, LaneEvent, Queued, Snapshot, beatsUntil, isSounding, phaseOf, queuedStates, windowOf } from './live';

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
  /** Every event drawn in its lanes, lit while it sounds. */
  events: NoteElement[];
  /** The countdown to a queued start or stop, if there is one. */
  eta: HTMLElement | null;
  /** How loud the clip is. */
  bar: Bar;
  active: Set<HTMLElement>;
  /** The whole cycle it was last in, so the clip can pulse as each begins. */
  cycle: number;
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
  /** The clips opened to show their lanes and the stack of changes made to them. */
  private open = new Set<string>();
  private queued = new Map<string, Queued>();
  private buses = new Map<string, Bar>();
  private frameAt = performance.now();

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
    this.queued = queuedStates(snapshot);
    const signature = JSON.stringify([
      snapshot.connected,
      Object.keys(snapshot.levels.buses),
      snapshot.scenes.map(scene => scene.name),
      snapshot.clips.map(clip => [
        clip.name,
        clip.instrument,
        clip.running,
        clip.muted,
        clip.soloed,
        this.queued.get(clip.name)?.kind,
        clip.edits,
        clip.length,
        clip.endless,
        this.open.has(clip.name)
          ? clip.lanes.map(lane => [
              lane.name,
              lane.truncated,
              lane.events.map(event => [event.start, event.dur, event.value, event.gain, event.pan]),
            ])
          : null,
      ]),
    ]);
    if (signature !== this.signature) {
      this.signature = signature;
      this.rebuild(snapshot);
    }
    for (const clip of snapshot.clips) {
      const row = this.rows.get(clip.name);
      if (row) {
        row.clip = clip;
        row.bar.push(snapshot.levels.nodes[clip.name] ?? 0);
      }
    }
    for (const [name, bar] of this.buses) bar.push(snapshot.levels.buses[name] ?? 0);
  }

  private rebuild(snapshot: Snapshot) {
    this.rows.clear();
    this.buses.clear();
    this.root.replaceChildren();
    if (!snapshot.connected) {
      const empty = element('div', 'stage-empty');
      empty.innerHTML =
        '<strong>No engine yet</strong><span>The stage looks for one on the port above. Start one with <code>niminal server</code>, then connect.</span>';
      const connect = button('Connect', 'Look for the engine again', 'connect');
      connect.classList.add('ready-connect');
      connect.addEventListener('click', () => this.onAction('connect'));
      empty.append(connect);
      this.root.append(empty);
      return;
    }
    if (snapshot.clips.length === 0) {
      const empty = element('div', 'stage-empty ready');
      empty.innerHTML = '<strong>Ready</strong><span>Nothing has been sent yet. Evaluate your code to put its clips here.</span>';
      empty.append(this.shortcutList());
      this.root.append(empty);
      return;
    }
    if (!snapshot.clips.some(clip => clip.running)) {
      const scene = snapshot.scenes[0];
      const next = scene ? `Press <kbd>${mod}1</kbd> to start <b>${scene.name}</b>` : 'Press ▶ on a clip to start it';
      const hint = element('div', 'stage-ready');
      hint.innerHTML = `Nothing is playing yet · ${next}`;
      this.root.append(hint);
    }
    snapshot.clips.forEach((clip, index) => this.root.append(this.row(clip, index)));
    const buses = Object.keys(snapshot.levels.buses).sort();
    if (buses.length) {
      const section = element('section', 'buses');
      section.append(text('div', 'buses-title', 'buses'));
      for (const name of buses) {
        const line = element('div', 'bus');
        const bar = new Bar();
        this.buses.set(name, bar);
        line.append(text('span', 'bus-name', name), bar.element);
        section.append(line);
      }
      this.root.append(section);
    }
  }

  /** The shortcuts, as a grid of keys and what they do. */
  private shortcutList() {
    const list = element('dl', 'shortcut-list');
    for (const [keys, what] of shortcuts) {
      list.append(text('dt', '', keys), text('dd', '', what));
    }
    return list;
  }

  private row(clip: Clip, index: number) {
    const article = element('article', 'clip');
    // the same colour as the clip's notes in the code
    article.style.setProperty('--clip', CLIP_COLOURS[index % CLIP_COLOURS.length]);
    article.dataset.clip = clip.name;
    article.classList.toggle('running', clip.running);
    article.classList.toggle('muted', clip.muted);
    article.classList.toggle('soloed', clip.soloed);
    article.classList.toggle('idle', !clip.running);
    const open = this.open.has(clip.name);
    article.classList.toggle('open', open);
    const queued = this.queued.get(clip.name);
    if (queued) article.classList.add(`queued-${queued.kind}`);

    // one line: what it is, what has been done to it, and the controls that matter live
    const head = element('header', 'clip-head');
    head.title = open ? 'Click to close' : 'Click for its lanes and the changes made to it';
    // the chevron says the line opens; whether it is playing shows in its colour, its name and its stop button
    head.append(text('span', 'clip-twisty', '▸'), text('span', 'clip-name', clip.name));

    const stack = element('span', 'clip-edits');
    clip.edits.forEach((edit, i) => {
      const newest = i === clip.edits.length - 1;
      const chip = text('span', newest ? 'chip newest' : 'chip', edit);
      chip.title = newest ? `${clip.name}.undo takes this back first` : `Change ${i + 1}: ${edit}`;
      stack.append(chip);
    });
    head.append(stack, element('span', 'clip-spacer'));
    const bar = new Bar('clip-meter');
    head.append(bar.element);
    let eta: HTMLElement | null = null;
    if (queued) {
      eta = text('span', 'queued-eta', '');
      eta.title = `${queued.kind === 'start' ? 'Starts' : 'Stops'} at the next boundary`;
      head.append(eta);
    }

    const controls = element('span', 'clip-controls');
    const running = button(
      clip.running ? '■' : '▶',
      clip.running ? `Stop ${clip.name}` : `Start ${clip.name}`,
      clip.running ? 'stop' : 'start',
    );
    running.classList.add('play');
    controls.append(running);
    const mute = button('M', clip.muted ? 'Unmute this clip' : 'Mute this clip', clip.muted ? 'unmute' : 'mute');
    mute.classList.toggle('on', clip.muted);
    const solo = button('S', clip.soloed ? 'Unsolo this clip' : 'Solo this clip', clip.soloed ? 'unsolo' : 'solo');
    solo.classList.toggle('on', clip.soloed);
    controls.append(mute, solo);
    head.append(controls);
    head.addEventListener('click', () => {
      if (this.open.has(clip.name)) this.open.delete(clip.name);
      else this.open.add(clip.name);
      if (this.snapshot) this.rebuild(this.snapshot);
    });
    article.append(head);

    const events: NoteElement[] = [];
    const playhead = element('div', 'playhead');
    if (open) {
      const detail = element('div', 'clip-detail');
      const loop = clip.endless
        ? 'endless'
        : `${clip.length.toFixed(clip.length % 1 === 0 ? 0 : 2)} bar${clip.length === 1 ? '' : 's'}`;
      detail.append(text('div', 'clip-meta', `${clip.instrument || 'no instrument'} · ${loop}`));
      detail.append(this.undoStack(clip));

      const lanes = element('div', 'clip-lanes');
      const window = windowOf(clip);
      const noteLane = clip.lanes.find(lane => lane.name === 'notes') ?? clip.lanes[0];
      if (!noteLane) {
        lanes.append(text('div', 'clip-empty', clip.running ? 'playing from keys' : 'no lanes'));
      } else {
        lanes.append(this.lane(noteLane, window, true, clip, events));
        for (const lane of clip.lanes) {
          if (lane !== noteLane) lanes.append(this.lane(lane, window, false, clip, events));
        }
      }
      playhead.append(element('span', 'playhead-dot'));
      const gates = element('div', 'clip-gates');
      gates.append(playhead);
      lanes.append(gates);
      detail.append(lanes);
      article.append(detail);
    }

    article.querySelectorAll<HTMLElement>('button[data-action]').forEach(node => {
      node.addEventListener('click', event => {
        event.stopPropagation();
        const action = node.dataset.action ?? '';
        this.onAction(action, clip.name, node.dataset.change);
      });
    });

    const row: Row = { clip, element: article, playhead, events, eta, bar, active: new Set(), cycle: -1 };
    this.rows.set(clip.name, row);
    return article;
  }

  /** What has been done to a clip, newest first: the order `undo` takes the changes back in. */
  private undoStack(clip: Clip) {
    const box = element('div', 'undo-stack');
    if (clip.edits.length === 0) {
      box.append(text('div', 'undo-empty', 'No changes: this is the clip as written'));
      return box;
    }
    box.append(text('div', 'undo-title', 'Changes · undo takes the top one first'));
    [...clip.edits].reverse().forEach((edit, i) => {
      const number = clip.edits.length - i;
      const line = element('div', i === 0 ? 'undo-line next' : 'undo-line');
      line.append(text('span', 'undo-number', String(number)), text('span', 'undo-edit', edit));
      if (i === 0) line.append(text('span', 'undo-note', '← next undo'));
      box.append(line);
    });
    box.append(text('div', 'undo-line base', 'as written'));
    const actions = element('div', 'undo-actions');
    actions.append(
      button('Undo last', `${clip.name}.undo`, 'edit', { change: 'undo' }),
      button('Clear all', `${clip.name}.clear`, 'edit', { change: 'clear' }),
    );
    box.append(actions);
    return box;
  }

  private lane(lane: Lane, window: number, notes: boolean, clip: Clip, events: NoteElement[]) {
    const wrap = element('div', notes ? 'lane notes-lane' : 'lane param-lane');
    wrap.append(text('span', 'lane-name', lane.name));
    const track = element('div', 'lane-track');
    const cycles = Math.max(1, window);
    track.style.backgroundSize = `${100 / cycles}% 100%`;
    for (const event of lane.events) {
      const node = bar(lane, event, window, notes);
      events.push({ element: node, start: event.start, dur: event.dur });
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
    const now = performance.now();
    const passed = Math.min((now - this.frameAt) / 1000, 0.25);
    this.frameAt = now;
    for (const bar of [...[...this.rows.values()].map(row => row.bar), ...this.buses.values()]) {
      bar.step(passed);
      bar.draw();
    }
    const snapshot = this.snapshot;
    if (!snapshot?.transport) return;
    const bpm = snapshot.transport.bpm;
    const beats = snapshot.beats_per_bar || 4;
    const elapsed = (performance.now() - this.updatedAt) / 1000;
    for (const row of this.rows.values()) {
      const clip = row.clip;
      const queued = row.eta && this.queued.get(clip.name);
      if (row.eta && queued) row.eta.textContent = `${queued.kind === 'start' ? '▶' : '■'} in ${beatsUntil(snapshot, queued.landsAt, elapsed).toFixed(1)}`;
      if (!clip.running || clip.cycles == null) {
        row.playhead.style.opacity = '0';
        for (const node of row.active) node.classList.remove('active');
        row.active.clear();
        continue;
      }
      const cycles = clip.cycles + (elapsed * bpm) / 60 / beats;
      const phase = phaseOf(clip, cycles);
      const window = windowOf(clip);
      const cycle = Math.floor(cycles);
      if (cycle !== row.cycle) {
        row.cycle = cycle;
        row.element.classList.remove('pulse');
        void row.element.offsetWidth;
        row.element.classList.add('pulse');
      }
      row.playhead.style.opacity = '1';
      row.playhead.style.left = `${(phase / window) * 100}%`;
      const active = new Set<HTMLElement>();
      for (const note of row.events) {
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
