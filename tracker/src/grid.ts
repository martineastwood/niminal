import { laneParameter, targetKey, formatValue, parseValue, envelopeValue } from './automation';
import { keyboardNote, transposeNote, emptyFxLane, blankColumn, subColumns, maxNoteColumns, type FxTarget, type EffectParameters, type Project, type Pattern, type Track, type SubColumn } from './project';
import { NoteHistory, parseNotes, type NoteEdit } from './history';

/** What the grid asks of the sequence: move between steps, insert or remove one, or point the step at another pattern. */
export type SequenceAction = 'previous' | 'next' | 'first' | 'last' | 'new' | 'remove' | 'pattern-previous' | 'pattern-next';

const ROW_WIDTH = 30, NOTE_WIDTH = 44, SUB_WIDTH = 26, LANE_WIDTH = 100;

/** A column of cells. Note-family lanes have `fx` null; parameter lanes have a `kind` of null. */
type Kind = 'note' | SubColumn;
interface Lane { track: number; kind: Kind | null; col: number; fx: number | null; values: (string | null)[] }
const subLabel: Record<SubColumn, string> = { inst: 'Ins', vol: 'Vol', pan: 'Pan', delay: 'Dly' };
const subTitle: Record<SubColumn, string> = {
  inst: 'Instrument for this note: its number in the sidebar list',
  vol: 'Volume for this note, 0 to 100',
  pan: 'Panning for this note, -50 (left) to +50 (right), C for centre',
  delay: 'Delay: how far into the row the note starts, as a percent, 0 to 99',
};
const subRange: Record<SubColumn, string> = { inst: 'an instrument number', vol: '0 to 100', pan: '-50 to 50, or C', delay: '0 to 99' };

// Cells store the engine's whole numbers (volume and pan 0 to 128, delay 0 to 255); these show them as people count.
/** What a sub-column cell reads as, for display and the clipboard. */
export function subText(kind: SubColumn, value: string | null, project: Project): string {
  if (value === null) return '';
  const n = Number(value);
  if (kind === 'inst') { const index = project.definitions.filter(d => d.kind === 'instr').findIndex(d => d.id === value); return index < 0 ? '??' : String(index).padStart(2, '0'); }
  if (kind === 'vol') return String(Math.round(n * 100 / 128));
  if (kind === 'delay') return String(Math.min(99, Math.round(n * 100 / 256)));
  const pan = Math.round((n - 64) * 50 / 64);
  return pan === 0 ? 'C' : pan > 0 ? `+${pan}` : String(pan);
}
/** The stored value for typed or pasted text, or null when it names nothing. */
export function subValue(kind: SubColumn, text: string, project: Project): string | null {
  const t = text.trim().replace(/%$/, '');
  if (kind === 'pan' && /^c$/i.test(t)) return '64';
  if (!/^[+-]?\d{1,3}$/.test(t)) return null;
  const n = Number(t);
  if (kind === 'inst') return n >= 0 && /^\d+$/.test(t) ? project.definitions.filter(d => d.kind === 'instr')[n]?.id ?? null : null;
  if (kind === 'vol') return n >= 0 && n <= 100 && /^\d+$/.test(t) ? String(Math.round(n * 1.28)) : null;
  if (kind === 'delay') return n >= 0 && n <= 99 && /^\d+$/.test(t) ? String(Math.round(n * 2.56)) : null;
  return n >= -50 && n <= 50 ? String(Math.round(64 + n * 64 / 50)) : null;
}

export class PatternGrid {
  private row = 0;
  private column = 0;
  private anchor = { row: 0, column: 0 };
  private histories = new Map<string, NoteHistory>();
  private targetSignatures = new Map<string, string>();
  private playing: number | null = null;
  private pad = 0;
  private origin = 0;
  private shown = -1;
  private animation = 0;
  private playhead = document.createElement('div');
  private frame = document.createElement('div');
  /** Record mode: typed keys enter notes and values. Otherwise they only preview the instrument. */
  recording = false;
  modeChanged: (recording: boolean) => void = () => {};
  /** Set by the owner: why a track's cells in the selected sequence step are not played, if they are not. */
  slot: (track: number) => { kind: 'mute' | 'alias'; text: string } | null = () => null;
  private dragging = false;
  private dragged = false;

  constructor(
    private parent: HTMLElement,
    private project: () => Project,
    private pattern: () => Pattern,
    private octave: () => number,
    private edited: () => void,
    private audition: (id: string, note: string, track: number) => void,
    private togglePlay: () => void,
    private trackSelected: (track: number) => void = () => {},
    private openEffects: (track: number, sameTrack: boolean) => void = () => {},
    private step: () => number = () => 1,
    private parameters: () => EffectParameters = () => ({}),
    private feedback: (text: string) => void = () => {},
    private openEnvelope: (track: number, fx: number) => void = () => {},
    private sequence: (action: SequenceAction) => void = () => {},
    /** The instrument that notes entered now are given: the one chosen in the sound browser. */
    private currentInstrument: () => string | null = () => null,
  ) {
    this.playhead.className = 'playhead'; this.playhead.hidden = true; this.frame.className = 'track-frame'; this.frame.hidden = true;
    parent.parentElement?.append(this.playhead, this.frame);
    parent.addEventListener('scroll', () => this.placeFrame());
    if (typeof ResizeObserver !== 'undefined') new ResizeObserver(() => this.placeFrame()).observe(parent);
    parent.addEventListener('pointerdown', event => {
      const cell = (event.target as HTMLElement).closest<HTMLElement>('[data-column]');
      if (!cell || event.button !== 0 || (event.target as HTMLElement).closest('input, .value-drag')) return;
      event.preventDefault(); this.dragging = true; this.dragged = false;
      this.row = Number(cell.dataset.noteRow); this.column = Number(cell.dataset.column);
      if (!event.shiftKey) this.anchor = { row: this.row, column: this.column };
      parent.setPointerCapture?.(event.pointerId); parent.focus(); this.trackSelected(this.activeTrack); this.paintSelection();
    });
    parent.addEventListener('pointermove', event => {
      if (!this.dragging) return;
      const cell = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>('[data-column]');
      if (!cell || !parent.contains(cell)) return;
      const row = Number(cell.dataset.noteRow), column = Number(cell.dataset.column);
      if (row === this.row && column === this.column) return;
      this.dragged = true; this.row = row; this.column = column; this.trackSelected(this.activeTrack); this.paintSelection();
      cell.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    });
    for (const name of ['pointerup', 'pointercancel', 'lostpointercapture']) parent.addEventListener(name, () => { this.dragging = false; });
    parent.addEventListener('keydown', event => this.keydown(event));
    parent.addEventListener('copy', event => this.copy(event));
    parent.addEventListener('cut', event => { if ((event.target as HTMLElement).closest('input')) return; this.copy(event); if (this.recording) this.clearSelection(); });
    parent.addEventListener('paste', event => {
      if ((event.target as HTMLElement).closest('input') || !this.recording) return;
      const rows = parseNotes(event.clipboardData?.getData('text/plain') ?? ''); event.preventDefault();
      if (!rows) { this.feedback('Clipboard needs notes or parameter values, separated by tabs.'); return; }
      const lanes = this.lanes, edits: NoteEdit[] = [];
      for (const [r, values] of rows.entries()) for (const [c, raw] of values.entries()) {
        const row = Math.min(this.row, this.anchor.row) + r, column = Math.min(this.column, this.anchor.column) + c;
        if (row >= this.pattern().rows || column >= lanes.length) continue;
        if (lanes[column].fx !== null && this.pattern().fx[lanes[column].track][lanes[column].fx!].envelope?.enabled) { this.feedback('Disable Use envelope before pasting row values into this lane.'); return; }
        let value = raw;
        if (value !== null) {
          const lane = lanes[column];
          if (lane.fx !== null) { const info = this.info(lane); value = info ? parseValue(value, info) : null; }
          else if (lane.kind === 'note') value = /^([A-G][#-][0-8]|OFF)$/.test(value) ? value : null;
          else value = subValue(lane.kind!, value, this.project());
          if (value === null) { this.feedback('Paste needs values in the destination lane’s units and range.'); return; }
        }
        edits.push({ row, track: column, before: lanes[column].values[row], after: value });
      }
      this.apply(edits);
    });
  }

  private get lanes(): Lane[] {
    const pattern = this.pattern();
    return this.project().tracks.flatMap((t, track): Lane[] => [
      ...pattern.notes[track].flatMap((column, col): Lane[] => [
        { track, kind: 'note', col, fx: null, values: column.notes },
        ...subColumns.filter(kind => shows(t, kind)).map((kind): Lane => ({ track, kind, col, fx: null, values: column[kind] })),
      ]),
      ...pattern.fx[track].map((lane, fx): Lane => ({ track, kind: null, col: 0, fx, values: lane.values })),
    ]);
  }
  private get values() { return this.lanes.map(lane => lane.values); }
  get activeTrack() { return this.lanes[this.column]?.track ?? 0; }
  private get history() {
    const pattern = this.pattern(), id = pattern.id;
    const signature = JSON.stringify([pattern.fx.map(track => track.map(lane => lane.target)), pattern.notes.map(columns => columns.length), this.project().tracks.map(t => t.show)]);
    if (this.targetSignatures.get(id) !== signature) this.histories.delete(id);
    this.targetSignatures.set(id, signature);
    if (!this.histories.has(id)) this.histories.set(id, new NoteHistory());
    return this.histories.get(id)!;
  }
  resetHistory() { this.histories.clear(); this.anchor = { row: this.row, column: this.column }; }
  undo() { if (this.history.undo(this.values)) { this.render(); this.edited(); } }
  redo() { if (this.history.redo(this.values)) { this.render(); this.edited(); } }
  private apply(edits: NoteEdit[]) { if (this.history.apply(this.values, edits)) { this.render(); this.edited(); } }

  selectTrack(track: number, render = true) {
    const index = Math.max(0, Math.min(this.project().tracks.length - 1, track));
    this.column = this.lanes.findIndex(lane => lane.track === index);
    this.anchor = { row: this.row, column: this.column }; this.trackSelected(index);
    if (render) this.render();
    else { this.paintSelection(); for (const header of this.parent.querySelectorAll<HTMLElement>('.track-header')) header.classList.toggle('selected-track', Number(header.dataset.track) === index); }
  }
  private id(row: number, lane: Lane) {
    if (lane.fx !== null) return `fx-${row}-${lane.track}-${lane.fx}`;
    return lane.kind === 'note' && lane.col === 0 ? `cell-${row}-${lane.track}` : `cell-${row}-${lane.track}-${lane.col}-${lane.kind}`;
  }
  private text(lane: Lane, value: string | null) { return lane.kind === 'note' || lane.kind === null ? value ?? '' : subText(lane.kind, value, this.project()); }
  /** The width of a track: its note columns with their sub-columns, then its parameter lanes. */
  private trackWidth(track: number) {
    const subs = subColumns.filter(kind => shows(this.project().tracks[track], kind)).length;
    return Math.max(150, this.pattern().notes[track].length * (NOTE_WIDTH + subs * SUB_WIDTH) + 2 + this.pattern().fx[track].length * LANE_WIDTH);
  }
  private selected(row: number, column: number) {
    return row >= Math.min(this.row, this.anchor.row) && row <= Math.max(this.row, this.anchor.row) && column >= Math.min(this.column, this.anchor.column) && column <= Math.max(this.column, this.anchor.column);
  }
  private paintSelection() {
    for (const cell of this.parent.querySelectorAll<HTMLElement>('[data-column]')) {
      const row = Number(cell.dataset.noteRow), column = Number(cell.dataset.column), selected = this.selected(row, column);
      cell.classList.toggle('range-selected', selected); cell.setAttribute('aria-selected', String(selected));
      cell.classList.toggle('cursor', row === this.row && column === this.column);
    }
    for (const header of this.parent.querySelectorAll<HTMLElement>('[data-track]')) header.classList.toggle('selected-track', Number(header.dataset.track) === this.activeTrack);
    this.parent.setAttribute('aria-activedescendant', this.id(this.row, this.lanes[this.column])); this.placeFrame();
  }

  render() {
    const project = this.project(), pattern = this.pattern(), lanes = this.lanes;
    this.column = Math.min(this.column, lanes.length - 1); this.anchor.column = Math.min(this.anchor.column, lanes.length - 1);
    this.row = Math.min(this.row, pattern.rows - 1); this.anchor.row = Math.min(this.anchor.row, pattern.rows - 1);
    this.parent.replaceChildren();
    this.parent.style.setProperty('--track-columns', `${ROW_WIDTH}px ${project.tracks.map((_, track) => `${this.trackWidth(track)}px`).join(' ')}`);
    const header = document.createElement('div'); header.className = 'grid-row grid-header'; header.setAttribute('role', 'row');
    const label = document.createElement('span'); label.setAttribute('role', 'columnheader'); header.append(label);
    for (const [index, track] of project.tracks.entries()) {
      const cell = document.createElement('div'); cell.setAttribute('role', 'columnheader'); cell.className = 'track-header'; cell.dataset.track = String(index);
      const slot = this.slot(index); if (slot) { cell.dataset.slot = slot.kind; cell.title = slot.text; }
      cell.style.setProperty('--track-colour', trackColour(track, index)); cell.classList.toggle('selected-track', index === this.activeTrack);
      const strip = document.createElement('button'); strip.className = 'track-colour'; strip.title = 'Track colour'; strip.setAttribute('aria-label', `${track.name} colour`);
      strip.onclick = () => this.pickColour(track, strip);
      const heading = document.createElement('div'); heading.className = 'track-heading';
      const name = document.createElement('button'); name.className = 'track-name'; name.textContent = `${String(index + 1).padStart(2, '0')} ${track.name}`; name.title = 'Double-click to rename'; name.onclick = () => this.selectTrack(index, false);
      name.ondblclick = () => {
        const input = document.createElement('input'); input.className = 'track-rename'; input.value = track.name; input.maxLength = 64; input.setAttribute('aria-label', `Rename track ${index + 1}`);
        let finished = false;
        const finish = (commit: boolean) => {
          if (finished) return; finished = true;
          const value = input.value.trim();
          if (commit && value && value !== track.name) { track.name = value; this.edited(); }
          this.render(); this.parent.focus();
        };
        input.onkeydown = event => { event.stopPropagation(); if (event.key === 'Enter' || event.key === 'Escape') { event.preventDefault(); finish(event.key === 'Enter'); } };
        input.onblur = () => finish(true); name.replaceWith(input); input.focus(); input.select();
      };
      const tools = document.createElement('div'); tools.className = 'track-tools';
      const fxButton = document.createElement('button'); fxButton.textContent = 'FX'; fxButton.title = `Show ${track.name} effects`; fxButton.setAttribute('aria-label', fxButton.title);
      fxButton.onclick = () => { const same = this.activeTrack === index; this.selectTrack(index); this.openEffects(index, same); }; tools.append(fxButton);
      const mute = document.createElement('button'); mute.textContent = 'M'; mute.title = `Mute ${track.name}`; mute.setAttribute('aria-label', mute.title); mute.setAttribute('aria-pressed', String(track.muted));
      mute.onclick = () => { track.muted = !track.muted; this.edited(); this.render(); }; tools.append(mute);
      const more = document.createElement('button'); more.textContent = '⋯'; more.title = `${track.name} note columns`; more.setAttribute('aria-label', `${track.name} columns`);
      more.className = 'track-more'; more.onclick = () => { this.selectTrack(index, false); this.pickColumns(index, more); }; tools.append(more); heading.append(name, tools);
      const source = document.createElement('div'); source.className = 'track-source';
      const columns = document.createElement('div'); columns.className = 'track-columns';
      for (const lane of lanes.filter(lane => lane.track === index && lane.fx === null)) {
        const label = document.createElement('span'); label.textContent = lane.kind === 'note' ? 'Note' : subLabel[lane.kind!];
        label.className = lane.kind === 'note' ? 'col-label' : 'col-label sub-label'; if (lane.kind === 'note' && lane.col > 0) label.classList.add('col-start');
        if (lane.kind !== 'note') label.title = subTitle[lane.kind!]; columns.append(label);
      }
      for (const [column, lane] of pattern.fx[index].entries()) {
        const picker = document.createElement('select'); picker.className = 'lane-target'; picker.setAttribute('aria-label', `${track.name} parameter lane ${column + 1}`);
        picker.add(new Option('—', ''));
        const choices: FxTarget[] = [{ kind: 'volume' }, { kind: 'pan' }];
        for (const definition of project.definitions) if (definition.kind === 'instr') for (const p of this.parameters()[definition.id] ?? []) choices.push({ kind: 'instrument', instrument: definition.id, parameter: p.name });
        for (const device of track.fx) for (const p of this.parameters()[device.definition] ?? []) choices.push({ kind: 'effect', device: device.id, parameter: p.name });
        for (const target of choices) picker.add(new Option(laneParameter(target, index, project, this.parameters())!.label, targetKey(target)));
        if (lane.target && !choices.some(t => targetKey(t) === targetKey(lane.target))) picker.add(new Option(`Missing/unapplied parameter: ${lane.target.kind === 'effect' || lane.target.kind === 'instrument' ? lane.target.parameter : 'parameter'}`, targetKey(lane.target)));
        picker.value = targetKey(lane.target); picker.title = laneParameter(lane.target, index, project, this.parameters())?.label ?? 'Connect and Apply effects to expose more parameters';
        picker.onchange = () => {
          if (lane.values.some(value => value !== null) || lane.envelope?.points.length) { picker.value = targetKey(lane.target); this.feedback('Clear this lane’s values before choosing a different parameter.'); return; }
          const target: FxTarget | null = picker.value ? JSON.parse(picker.value) : null;
          if (target && pattern.fx[index].some(other => other !== lane && targetKey(other.target) === targetKey(target))) { picker.value = targetKey(lane.target); this.feedback('That parameter already has a lane in this track.'); return; }
          lane.target = target; this.resetHistory(); this.edited(); this.render();
        };
        const control = document.createElement('div'); control.className = 'lane-heading';
        const graph = document.createElement('button'); graph.textContent = '⌁'; graph.title = `Edit ${picker.title} envelope`; graph.setAttribute('aria-label', `${track.name} lane ${column + 1} automation`); graph.onclick = () => this.openEnvelope(index, column);
        control.append(picker, graph); columns.append(control);
      }
      const adjust = document.createElement('div'); adjust.className = 'fx-column-tools';
      for (const [delta, text] of [[-1, '−'], [1, '+']] as const) {
        const button = document.createElement('button'); button.textContent = text; button.title = `${delta > 0 ? 'Add' : 'Hide'} ${track.name} parameter lane`; button.setAttribute('aria-label', button.title);
        button.disabled = delta < 0 ? pattern.fx[index].length === 1 : pattern.fx[index].length === 4;
        button.onclick = () => {
          if (delta > 0) pattern.fx[index].push({ target: null, values: Array(Math.max(...pattern.notes.flat().map(column => column.notes.length))).fill(null) });
          else if ((pattern.fx[index].at(-1)!.values.some(value => value !== null) || pattern.fx[index].at(-1)!.envelope?.points.length)) { this.feedback('Clear the last parameter lane before hiding it.'); return; }
          else pattern.fx[index].pop();
          this.resetHistory(); this.selectTrack(index); this.edited();
        }; adjust.append(button);
      }
      source.append(adjust);
      cell.append(strip, heading, source, columns); cell.onkeydown = event => event.stopPropagation();
      cell.onclick = event => { if (!(event.target as HTMLElement).closest('button, select, input')) this.selectTrack(index, false); };
      header.append(cell);
    }
    this.parent.append(header, this.pads());
    for (let row = 0; row < pattern.rows; row++) {
      const line = document.createElement('div'); line.className = `grid-row ${row % project.rows_per_beat === 0 ? 'beat' : ''} ${row % (project.rows_per_beat * 4) === 0 ? 'bar' : ''}`; line.setAttribute('role', 'row'); line.dataset.row = String(row);
      const number = document.createElement('span'); number.className = 'row-number'; number.textContent = String(row).padStart(2, '0'); line.append(number);
      let column = 0;
      for (let track = 0; track < project.tracks.length; track++) {
        const group = document.createElement('div'); group.className = 'track-cells';
        for (const lane of lanes.filter(lane => lane.track === track)) {
          const c = column++, value = lane.values[row], cell = document.createElement('div'); cell.setAttribute('role', 'gridcell');
          cell.className = `${lane.fx !== null ? 'fx-command' : lane.kind === 'note' ? 'note' : 'sub'} ${value ? 'filled' : ''}`; cell.dataset.noteRow = String(row); cell.dataset.column = String(c);
          if (lane.kind === 'note' && lane.col > 0) cell.classList.add('col-start'); if (value === 'OFF') cell.classList.add('off');
          cell.id = this.id(row, lane); cell.textContent = value === null ? (lane.fx !== null ? '·' : lane.kind === 'note' ? '···' : '··') : lane.fx === null ? this.text(lane, value) : formatValue(value, this.info(lane));
          if (lane.fx !== null && pattern.fx[track][lane.fx].envelope?.enabled) { const envelope = pattern.fx[track][lane.fx].envelope!; const v = envelopeValue(envelope, row, this.info(lane)?.unit === 'db'); cell.textContent = v === null ? '·' : formatValue(v, this.info(lane)); cell.classList.add('envelope-cell'); }
          if (lane.fx !== null) { cell.title = `${this.info(lane)?.label ?? 'Choose a parameter above'} · Click to edit; drag ⋮ vertically to adjust. Blank holds the previous value.`; if (value !== null && this.info(lane) && !pattern.fx[track][lane.fx].envelope?.enabled) { const handle = document.createElement('span'); handle.className = 'value-drag'; handle.textContent = '⋮'; handle.title = 'Drag vertically to adjust'; handle.onpointerdown = event => this.dragValue(event, cell, lane, row, c); cell.append(handle); } }
          cell.onclick = event => {
            if (this.dragged) { this.dragged = false; return; }
            if (!event.shiftKey) this.anchor = { row, column: c };
            this.row = row; this.column = c; this.trackSelected(track); this.render(); this.parent.focus();
            if (!event.shiftKey && this.recording) { if (lane.fx !== null) this.editValue(row, c); else if (lane.kind !== 'note') this.editSub(row, c); }
          }; group.append(cell);
        }
        line.append(group);
      }
      this.parent.append(line);
    }
    this.parent.append(this.pads());
    this.paintSelection();
    const cursor = this.parent.querySelector<HTMLElement>(`#${this.id(this.row, lanes[this.column])}`);
    cursor?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    if (cursor) { const cell = cursor.getBoundingClientRect(), viewport = this.parent.getBoundingClientRect(); if (cell.left < viewport.left + ROW_WIDTH) this.parent.scrollLeft -= viewport.left + ROW_WIDTH - cell.left; }
    if (this.playing !== null && this.shown >= 0) this.followRow(this.shown);
  }

  /**
   * `row` is the row the engine is playing; `phase` (0 to 1) is how far through it was at `atMs`.
   * Together they pin the song's timeline, so row changes are predicted from the clock instead of
   * waiting for each status poll.
   */
  paint(row: number | null, phase: number | null = null, atMs = Date.now()) {
    const was = this.playing; this.playing = row;
    if (row === null) { this.playhead.hidden = true; this.shown = -1; if (this.pad) this.setPad(0); if (was !== null) this.placeFrame(); return; }
    if (was === null) this.placeFrame();
    this.origin = atMs - (row + (phase ?? 0)) * this.rowMs();
    this.animation ||= requestAnimationFrame(this.follow);
  }
  private rowMs() { return 60000 / (this.project().bpm * this.project().rows_per_beat); }
  private follow = () => {
    this.animation = 0;
    if (this.playing === null) return;
    const row = Math.min(Math.max(Math.floor((Date.now() - this.origin) / this.rowMs()), 0), this.pattern().rows - 1);
    if (row !== this.shown) { this.shown = row; this.followRow(row); }
    this.animation = requestAnimationFrame(this.follow);
  };
  /**
   * While playing, the pattern steps up one row at a time under a play bar that stays near the middle.
   * Padding above the first row and below the last lets the bar stay put at both ends.
   */
  private followRow(row: number) {
    const header = this.parent.querySelector<HTMLElement>('.grid-header')?.offsetHeight ?? 0;
    const pad = Math.round((this.parent.clientHeight - header) / 2); if (pad !== this.pad) this.setPad(pad);
    const top = this.contentTop(row); if (top === null) return;
    this.parent.scrollTop = top - header - pad;
    const y = top - this.parent.scrollTop;
    this.playhead.hidden = y < header || y > this.parent.clientHeight;
    this.playhead.style.top = `${y}px`;
  }
  /** A small panel under `anchor` that closes on a click elsewhere or Escape. */
  private popup(anchor: HTMLElement, className: string) {
    document.querySelector('.colour-popover')?.remove();
    const element = document.createElement('div'); element.className = `colour-popover ${className}`;
    const outside = (event: Event) => { if (!element.contains(event.target as Node)) close(); };
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') { event.stopPropagation(); close(); } };
    const close = () => { element.remove(); document.removeEventListener('pointerdown', outside, true); document.removeEventListener('keydown', escape, true); };
    const box = anchor.getBoundingClientRect(); element.style.left = `${Math.min(box.left, window.innerWidth - 200)}px`; element.style.top = `${box.bottom + 2}px`;
    document.body.append(element); document.addEventListener('pointerdown', outside, true); document.addEventListener('keydown', escape, true);
    return { element, close };
  }
  private pickColour(track: Track, anchor: HTMLElement) {
    const { element, close } = this.popup(anchor, 'colour-choices');
    const choose = (colour: string | undefined) => { close(); if (track.colour === colour) return; if (colour) track.colour = colour; else delete track.colour; this.edited(); this.render(); };
    for (const colour of trackPalette) {
      const swatch = document.createElement('button'); swatch.className = 'swatch'; swatch.style.background = colour; swatch.title = colour; swatch.setAttribute('aria-label', `Colour ${colour}`);
      swatch.onclick = () => choose(colour); element.append(swatch);
    }
    const custom = document.createElement('input'); custom.type = 'color'; custom.title = 'Custom colour'; custom.setAttribute('aria-label', 'Custom colour'); custom.value = track.colour ?? '#808080';
    custom.onchange = () => choose(custom.value); element.append(custom);
    const reset = document.createElement('button'); reset.className = 'swatch-reset'; reset.textContent = 'Default'; reset.onclick = () => choose(undefined); element.append(reset);
  }
  /** Note columns (shared by every pattern) and which sub-columns the track shows. */
  private pickColumns(index: number, anchor: HTMLElement) {
    const { element, close } = this.popup(anchor, 'column-choices');
    const project = this.project(), track = project.tracks[index];
    const changed = () => {
      this.resetHistory(); this.edited(); this.render(); close();
      const again = this.parent.querySelector<HTMLElement>(`.track-header[data-track="${index}"] .track-more`); if (again) this.pickColumns(index, again);
    };
    const count = this.pattern().notes[index].length;
    const heading = document.createElement('span'); heading.textContent = 'Note columns';
    const fewer = document.createElement('button'); fewer.textContent = '−'; fewer.setAttribute('aria-label', 'Fewer note columns'); fewer.disabled = count <= 1;
    fewer.onclick = () => {
      if (project.patterns.some(p => { const last = p.notes[index].at(-1)!; return [last.notes, last.inst, last.vol, last.pan, last.delay].some(cells => cells.some(v => v !== null)); })) { this.feedback('Clear the last note column in every pattern before removing it.'); return; }
      for (const p of project.patterns) p.notes[index].pop();
      changed();
    };
    const amount = document.createElement('strong'); amount.textContent = String(count);
    const more = document.createElement('button'); more.textContent = '+'; more.setAttribute('aria-label', 'More note columns'); more.disabled = count >= maxNoteColumns;
    more.onclick = () => { for (const p of project.patterns) p.notes[index].push(blankColumn(Math.max(p.rows, p.notes[index][0].notes.length))); changed(); };
    element.append(heading, fewer, amount, more);
    for (const kind of subColumns.filter(kind => kind !== 'inst')) {
      const label = document.createElement('label'); label.title = subTitle[kind];
      const box = document.createElement('input'); box.type = 'checkbox'; box.checked = !!track.show?.[kind]; box.setAttribute('aria-label', `Show ${subLabel[kind]} column`);
      box.onchange = () => { track.show = { ...track.show, [kind]: box.checked }; changed(); };
      label.append(box, { vol: 'Volume', pan: 'Panning', delay: 'Delay' }[kind]); element.append(label);
    }
  }
  /** Edits a sub-column cell in place, like a parameter lane: type a number, Enter to save and move down. */
  private editSub(row: number, column: number, initial?: string) {
    const lane = this.lanes[column], kind = lane.kind as SubColumn, cell = this.parent.querySelector<HTMLElement>(`#${this.id(row, lane)}`);
    if (!cell) return;
    const input = document.createElement('input'); input.className = 'lane-value'; input.setAttribute('aria-label', `${subLabel[kind]} row ${row}`);
    input.value = initial ?? this.text(lane, lane.values[row]); const originalText = input.value;
    cell.replaceChildren(input); input.focus(); if (initial === undefined) input.select();
    let finished = false;
    const finish = (commit: boolean, advance: boolean) => {
      if (finished) return;
      const blank = input.value.trim() === '';
      const value = initial === undefined && input.value === originalText ? lane.values[row] : blank ? null : subValue(kind, input.value, this.project());
      if (commit && !blank && value === null) { this.feedback(kind === 'inst' ? 'No instrument has that number.' : `Enter ${subRange[kind]}.`); if (advance) { input.focus(); return; } commit = false; }
      finished = true;
      if (commit) this.apply([{ row, track: column, before: lane.values[row], after: value }]);
      if (advance && commit) this.advance(); this.render(); if (advance) this.parent.focus();
    };
    input.onkeydown = event => { event.stopPropagation(); if (event.key === 'Enter' || event.key === 'Escape' || event.key === 'Tab') { event.preventDefault(); finish(event.key !== 'Escape', true); } };
    input.onblur = () => finish(true, false);
  }
  setRecording(on: boolean) { this.recording = on; this.modeChanged(on); }
  /** Corner brackets mark the selected track, from its header to the bottom of the grid. */
  private placeFrame() {
    const header = this.parent.querySelector<HTMLElement>(`.track-header[data-track="${this.activeTrack}"]`);
    if (!header) { this.frame.hidden = true; return; }
    const box = this.parent.getBoundingClientRect(), cell = header.getBoundingClientRect();
    const left = Math.max(cell.left - box.left, ROW_WIDTH), right = Math.min(cell.right - box.left, this.parent.clientWidth);
    this.frame.hidden = right - left < 12;
    const end = this.parent.querySelector<HTMLElement>(`[data-row="${this.pattern().rows - 1}"]`)?.getBoundingClientRect().bottom;
    // While playing the pattern scrolls past, so the bottom corners stay at the foot of the view.
    const bottom = this.playing !== null || end === undefined ? this.parent.clientHeight : Math.min(this.parent.clientHeight, end - box.top);
    this.frame.style.cssText = `left:${left}px;width:${right - left}px;top:0;height:${Math.max(0, bottom)}px;--c:${trackColour(this.project().tracks[this.activeTrack], this.activeTrack)};--side:${header.offsetHeight}px`;
  }
  private setPad(pad: number) { this.pad = pad; for (const spacer of this.parent.querySelectorAll<HTMLElement>('.grid-pad')) spacer.style.height = `${pad}px`; }
  private pads() { const spacer = document.createElement('div'); spacer.className = 'grid-pad'; spacer.style.height = `${this.pad}px`; return spacer; }
  private contentTop(row: number) {
    const line = this.parent.querySelector<HTMLElement>(`[data-row="${row}"]`); if (!line) return null;
    return line.getBoundingClientRect().top - this.parent.getBoundingClientRect().top + this.parent.scrollTop;
  }
  private copy(event: ClipboardEvent) {
    if ((event.target as HTMLElement).closest('input')) return;
    const rows: string[] = [], lanes = this.lanes;
    for (let row = Math.min(this.row, this.anchor.row); row <= Math.max(this.row, this.anchor.row); row++) {
      const values: string[] = [];
      for (let c = Math.min(this.column, this.anchor.column); c <= Math.max(this.column, this.anchor.column); c++) {
        const lane = lanes[c], envelope = lane.fx === null ? undefined : this.pattern().fx[lane.track][lane.fx].envelope;
        const value = envelope?.enabled ? envelopeValue(envelope, row, this.info(lane)?.unit === 'db') : lane.values[row];
        values.push(value === null ? '---' : lane.fx === null ? this.text(lane, String(value)) : formatValue(value, this.info(lane), true));
      }
      rows.push(values.join('\t'));
    }
    event.clipboardData?.setData('text/plain', rows.join('\n')); event.preventDefault();
  }
  /** The selected rectangle: first and last row and column. */
  private get bounds() {
    return { top: Math.min(this.row, this.anchor.row), bottom: Math.max(this.row, this.anchor.row), left: Math.min(this.column, this.anchor.column), right: Math.max(this.column, this.anchor.column) };
  }
  /** Lanes drawn by a graphical envelope have no cells of their own to edit. */
  private editable(lane: Lane) { return lane.fx === null || !this.pattern().fx[lane.track][lane.fx].envelope?.enabled; }
  /** Moves the selected notes up or down by `semitones`; OFF and notes that would leave the range stay as they are. */
  private transpose(semitones: number) {
    const { top, bottom, left, right } = this.bounds, lanes = this.lanes, edits: NoteEdit[] = [];
    for (let column = left; column <= right; column++) {
      if (lanes[column].kind !== 'note') continue;
      for (let row = top; row <= bottom; row++) {
        const before = lanes[column].values[row], after = before === null ? null : transposeNote(before, semitones);
        if (after) edits.push({ row, track: column, before, after });
      }
    }
    this.apply(edits);
  }
  /** Fills the rows between the first and last selected row with a straight line between their values, in each column of numbers. */
  private interpolate() {
    const { top, bottom, left, right } = this.bounds, lanes = this.lanes, edits: NoteEdit[] = [];
    for (let column = left; column <= right; column++) {
      const lane = lanes[column];
      if (lane.kind === 'note' || lane.kind === 'inst' || !this.editable(lane)) continue;
      const from = Number(lane.values[top]), to = Number(lane.values[bottom]);
      if (lane.values[top] === null || lane.values[bottom] === null || !Number.isFinite(from) || !Number.isFinite(to)) continue;
      for (let row = top + 1; row < bottom; row++) {
        const value = from + (to - from) * (row - top) / (bottom - top);
        edits.push({ row, track: column, before: lane.values[row], after: String(lane.fx === null ? Math.round(value) : Number(value.toPrecision(8))) });
      }
    }
    if (bottom - top < 2 || !edits.length) { this.feedback('Interpolate needs three or more rows selected, with values at the first and last row of a volume, pan, delay or parameter column.'); return; }
    this.apply(edits);
  }
  /** Inserts a blank row at the top of the selection, or deletes that row, shifting the rest of each selected track (or just the selected columns) up or down. */
  private shiftRows(direction: 1 | -1, scope: 'track' | 'column' = 'track') {
    const { top, left, right } = this.bounds, lanes = this.lanes, rows = this.pattern().rows, edits: NoteEdit[] = [];
    const tracks = new Set(lanes.slice(left, right + 1).map(lane => lane.track));
    lanes.forEach((lane, column) => {
      if ((scope === 'track' ? !tracks.has(lane.track) : column < left || column > right) || !this.editable(lane)) return;
      for (let row = top; row < rows; row++) {
        const source = direction === 1 ? (row === top ? null : lane.values[row - 1]) : row + 1 < rows ? lane.values[row + 1] : null;
        edits.push({ row, track: column, before: lane.values[row], after: source });
      }
    });
    this.apply(edits);
  }
  /** Copies the selection onto the rows just below it and selects the copy, so repeating it carries on down the pattern. */
  private duplicate() {
    const { top, bottom, left, right } = this.bounds, lanes = this.lanes, rows = this.pattern().rows, height = bottom - top + 1, edits: NoteEdit[] = [];
    if (bottom + 1 >= rows) { this.feedback('There is no room below the selection.'); return; }
    for (let column = left; column <= right; column++) {
      if (!this.editable(lanes[column])) continue;
      for (let row = top; row <= bottom && row + height < rows; row++) edits.push({ row: row + height, track: column, before: lanes[column].values[row + height], after: lanes[column].values[row] });
    }
    this.apply(edits);
    const shift = Math.min(height, rows - 1 - bottom); this.row += shift; this.anchor.row += shift;
    this.render();
  }
  private clearSelection() {
    const edits: NoteEdit[] = [];
    this.lanes.forEach((lane, column) => { if (lane.fx !== null && this.pattern().fx[lane.track][lane.fx].envelope?.enabled) return; for (let row = 0; row < this.pattern().rows; row++) if (this.selected(row, column)) edits.push({ row, track: column, before: lane.values[row], after: null }); }); this.apply(edits);
  }
  private advance() { this.row = (this.row + Math.max(0, Math.min(256, Math.trunc(this.step()) || 0))) % this.pattern().rows; this.anchor = { row: this.row, column: this.column }; }
  private info(lane: Lane) { return laneParameter(lane.fx === null ? null : this.pattern().fx[lane.track][lane.fx].target, lane.track, this.project(), this.parameters()); }
  private editValue(row: number, column: number, initial?: string) {
    const lane = this.lanes[column];
    if (lane.fx !== null && this.pattern().fx[lane.track][lane.fx].envelope?.enabled) { this.openEnvelope(lane.track, lane.fx); return; }
    const info = this.info(lane), cell = this.parent.querySelector<HTMLElement>(`#${this.id(row, lane)}`);
    if (!info || !cell) { this.feedback('Choose a lane parameter first. Connect and Apply an effect to expose its controls.'); return; }
    const input = document.createElement('input'); input.className = 'lane-value'; input.setAttribute('aria-label', `${info.label} row ${row}`);
    input.value = initial ?? (lane.values[row] === null ? '' : formatValue(lane.values[row]!, info));
    const originalText = input.value;
    input.placeholder = formatValue(info.default, info); cell.replaceChildren(input); input.focus(); if (initial === undefined) input.select();
    let finished = false;
    const finish = (commit: boolean, advance: boolean) => {
      if (finished) return;
      const value = initial === undefined && input.value === originalText ? lane.values[row] : input.value.trim() === '' ? null : parseValue(input.value, info);
      if (commit && input.value.trim() && value === null) { this.feedback(`Enter ${formatValue(info.min, info)} to ${formatValue(info.max, info)}.`); if (advance) { input.focus(); return; } commit = false; }
      finished = true;
      if (commit) this.apply([{ row, track: column, before: lane.values[row], after: value }]);
      if (advance && commit) this.advance(); this.render(); if (advance) this.parent.focus();
    };
    input.onkeydown = event => { event.stopPropagation(); if (event.key === 'Enter' || event.key === 'Escape' || event.key === 'Tab') { event.preventDefault(); finish(event.key !== 'Escape', true); } };
    input.onblur = () => finish(true, false);
  }
  private dragValue(event: PointerEvent, cell: HTMLElement, lane: Lane, row: number, column: number) {
    const info = this.info(lane); if (!info) return;
    event.preventDefault(); event.stopPropagation();
    const handle = event.currentTarget as HTMLElement, before = lane.values[row], start = Number(before ?? info.default), y = event.clientY;
    let value = start; handle.setPointerCapture?.(event.pointerId);
    handle.onpointermove = move => { value = Math.min(info.max, Math.max(info.min, start + (y - move.clientY) * (info.max - info.min) / (move.shiftKey ? 2000 : 200))); cell.firstChild!.textContent = formatValue(value, info); };
    const finish = (commit: boolean) => { handle.onpointermove = null; handle.onpointerup = null; handle.onpointercancel = null; if (commit) this.apply([{ row, track: column, before, after: String(Number(value.toPrecision(8))) }]); else this.render(); };
    handle.onpointerup = up => { up.stopPropagation(); finish(true); }; handle.onpointercancel = () => finish(false); handle.onclick = click => click.stopPropagation();
  }
  writeParameter(track: number, target: FxTarget, value: number) {
    const columns = this.pattern().fx[track]; let fx = columns.findIndex(lane => targetKey(lane.target) === targetKey(target));
    if (fx < 0) {
      fx = columns.findIndex(lane => lane.values.every(value => value === null) && lane.target === null);
      if (fx < 0 && columns.length < 4) { fx = columns.length; columns.push(emptyFxLane(this.pattern().notes[track][0].notes.length)); }
      if (fx < 0) { this.feedback('Add or clear a parameter lane before recording this control (maximum four per track).'); return; }
      columns[fx].target = { ...target }; this.resetHistory();
    }
    const envelope = columns[fx].envelope; if (envelope) envelope.enabled = false;
    const column = this.lanes.findIndex(lane => lane.track === track && lane.fx === fx); this.column = column;
    this.apply([{ row: this.row, track: column, before: columns[fx].values[this.row], after: String(value) }]);
    this.advance(); this.render(); this.parent.focus();
  }
  private keydown(event: KeyboardEvent) {
    if ((event.target as HTMLElement).closest('input, select, textarea')) return;
    if (event.ctrlKey || event.metaKey) {
      const key = event.key.length === 1 ? event.key.toLowerCase() : event.key;
      if (key === 'z') { event.preventDefault(); if (event.shiftKey) this.redo(); else this.undo(); }
      if (key === 'y') { event.preventDefault(); this.redo(); }
      if (key === 'a') { event.preventDefault(); this.anchor = { row: 0, column: 0 }; this.row = this.pattern().rows - 1; this.column = this.lanes.length - 1; this.trackSelected(this.activeTrack); this.render(); }
      const sequence: Record<string, SequenceAction> = { ArrowUp: 'previous', ArrowDown: 'next', Home: 'first', End: 'last', ArrowLeft: 'pattern-previous', ArrowRight: 'pattern-next', ...(event.shiftKey ? {} : { Insert: 'new', Delete: 'remove' }) };
      const edit: Record<string, () => void> = { i: () => this.interpolate(), d: () => this.duplicate(), Enter: () => this.shiftRows(1), Backspace: () => this.shiftRows(-1), ...(event.shiftKey ? { Insert: () => this.shiftRows(1, 'column'), Delete: () => this.shiftRows(-1, 'column') } : {}) };
      if (sequence[key]) { event.preventDefault(); this.sequence(sequence[key]); }
      else if (edit[key]) { event.preventDefault(); if (this.recording) edit[key](); }
      return;
    }
    if (event.altKey && (event.key === 'ArrowUp' || event.key === 'ArrowDown')) {
      event.preventDefault(); if (this.recording) this.transpose((event.key === 'ArrowUp' ? 1 : -1) * (event.shiftKey ? 12 : 1)); return;
    }
    if (event.altKey) return;
    const lanes = this.lanes, lane = lanes[this.column];
    switch (event.key) {
      case ' ': event.preventDefault(); this.togglePlay(); return;
      case 'Escape': event.preventDefault(); this.setRecording(!this.recording); return;
      case 'ArrowUp': this.row = event.shiftKey ? Math.max(0, this.row - 1) : (this.row + this.pattern().rows - 1) % this.pattern().rows; break;
      case 'ArrowDown': this.row = event.shiftKey ? Math.min(this.pattern().rows - 1, this.row + 1) : (this.row + 1) % this.pattern().rows; break;
      case 'ArrowLeft': this.column = event.shiftKey ? Math.max(0, this.column - 1) : (this.column + lanes.length - 1) % lanes.length; break;
      case 'ArrowRight': this.column = event.shiftKey ? Math.min(lanes.length - 1, this.column + 1) : (this.column + 1) % lanes.length; break;
      case 'Tab': this.selectTrack((this.activeTrack + (event.shiftKey ? this.project().tracks.length - 1 : 1)) % this.project().tracks.length); event.preventDefault(); return;
      case 'PageUp': this.row = Math.max(0, this.row - this.project().rows_per_beat * 4); break;
      case 'PageDown': this.row = Math.min(this.pattern().rows - 1, this.row + this.project().rows_per_beat * 4); break;
      case 'Home': case 'F9': this.row = 0; break;
      case 'End': this.row = this.pattern().rows - 1; break;
      case 'F10': this.row = Math.floor(this.pattern().rows / 4); break;
      case 'F11': this.row = Math.floor(this.pattern().rows / 2); break;
      case 'F12': this.row = Math.floor(this.pattern().rows * 3 / 4); break;
      case 'Insert': if (!this.recording) return; event.preventDefault(); this.shiftRows(1); return;
      case 'Backspace': if (!this.recording) return; event.preventDefault(); this.shiftRows(-1); return;
      case 'Delete': if (!this.recording) return; event.preventDefault(); this.clearSelection(); return;
      default: {
        if (lane.fx !== null) {
          if (!this.recording) return;
          if (event.key === 'Enter' || event.key.length === 1 && /[0-9+.-]/.test(event.key)) { event.preventDefault(); this.editValue(this.row, this.column, event.key === 'Enter' ? undefined : event.key); } return;
        }
        if (lane.kind !== 'note') {
          if (!this.recording || event.metaKey || event.ctrlKey || !(event.key === 'Enter' || /^[0-9+-]$/.test(event.key) || lane.kind === 'pan' && /^c$/i.test(event.key))) return;
          event.preventDefault(); this.editSub(this.row, this.column, event.key === 'Enter' ? undefined : event.key); return;
        }
        const off = event.key === '1' || event.key === '`';
        const note = off ? 'OFF' : keyboardNote(event.key, this.octave()); if (!note) return;
        event.preventDefault();
        const column = this.pattern().notes[lane.track][lane.col], inst = off ? undefined : this.currentInstrument();
        const instrument = !off && (instrumentAt(column.inst, this.row) ?? inst);
        if (instrument) this.audition(instrument, note, lane.track);
        if (!this.recording) return;
        // a note is given the chosen instrument's number, which can be edited afterwards
        const instLane = this.lanes.findIndex(other => other.track === lane.track && other.col === lane.col && other.kind === 'inst');
        this.apply([{ row: this.row, track: this.column, before: lane.values[this.row], after: note }, ...(inst ? [{ row: this.row, track: instLane, before: column.inst[this.row], after: inst }] : [])]);
        this.advance(); this.render(); return;
      }
    }
    if (!event.shiftKey) this.anchor = { row: this.row, column: this.column };
    this.trackSelected(this.activeTrack); event.preventDefault(); this.render();
  }
}

/** Which sub-columns a track draws: the instrument's always, the others when asked for. */
export const shows = (track: Track, kind: SubColumn) => kind === 'inst' || !!track.show?.[kind];
/** The instrument playing at `row`: the one set on it, or the last one set above it in the column. */
export function instrumentAt(inst: (string | null)[], row: number): string | null {
  for (let at = row; at >= 0; at--) if (inst[at]) return inst[at];
  return null;
}

export const trackPalette = ['#c97979', '#d08a5a', '#c6a465', '#a9b966', '#73b48a', '#5fb3a6', '#6db4be', '#829bd1', '#8f8fd8', '#b28bce', '#cc8fb2', '#9aa0a8'];
/** A track's own colour if it has one, otherwise a default from its position. */
export function trackColour(track: Track, index: number) { return track.colour ?? trackPalette[index % 8]; }
