import { maxLanes, patternTiming, patternSeconds, patternRow, patternSwing } from './project';
import { targetKey } from './automation';
import { describeAnyCommand } from './describe';
import { columnCommands, commandProblem, commandValue, glideRetriggers, maxFxColumns, maxTrackCommandColumns, trackCommandProblem, trackCommandValue, keyboardNote, noteAt, noteNumber, transposeNote, blankColumn, subColumns, maxNoteColumns, columnCount, columnStrip, columnLanes, type FxTarget, type Project, type Song, type Pattern, type Track, type SubColumn } from './project';
import { NoteHistory, parseNotes, type NoteEdit } from './history';

/** What the grid asks of the sequence: move between steps, insert or remove one, or point the step at another pattern. */
export type SequenceAction = 'previous' | 'next' | 'first' | 'last' | 'new' | 'remove' | 'pattern-previous' | 'pattern-next';

const ROW_WIDTH = 30, NOTE_WIDTH = 44, SUB_WIDTH = 26, MIN_TRACK_WIDTH = 104, MIN_STRIP_WIDTH = 84, FX_WIDTH = 34;

/** A column of cells of a note column, or a track's command column. */
type Kind = 'note' | SubColumn | 'cmd';
/** `fx` is a note column's command column and `cmd` a track's; `idx` says which of them. */
interface Lane { track: number; kind: Kind; col: number; values: (string | null)[]; idx?: number }
const subLabel: Record<Kind, string> = { note: 'Note', inst: 'Ins', vol: 'Vel', pan: 'Pan', delay: 'Dly', fx: 'Fx', cmd: 'Cmd' };
const subTitle: Record<Exclude<Kind, 'note'>, string> = {
  cmd: 'Command: Lxx sets the volume (FF is +3 dB), Pxx the pan (80 is centre), and four hex digits such as 12FF set parameter 2 of effect 1 (FF is its most)',
  inst: 'Instrument for this note: its number in the sidebar list',
  vol: 'Velocity: how hard this note plays, 0 to 100. The track\'s own volume is an automation lane',
  pan: 'Panning for this note, -50 (left) to +50 (right), C for centre',
  delay: 'Delay: how far into the row the note starts, as a percent, 0 to 99',
  fx: 'Command: a letter and two hex digits, or four for an effect. S20 starts the sample partway in, B00 plays it backwards, G10 glides the held note to this one, U08 / D08 slide it up or down, R03 retriggers, C03 cuts after 3 of 12 ticks, Y80 plays it half the time, I20 / O20 fade the level; L80 sets the track volume, P80 its pan, 12FF parameter 2 of effect 1',
};
const subRange: Record<Exclude<Kind, 'note'>, string> = { cmd: 'Lxx, Pxx or four hex digits, such as 12FF', inst: 'an instrument number', vol: '0 to 100', pan: '-50 to 50, or C', delay: '0 to 99', fx: 'a letter and two hex digits, such as S20' };

// Cells store the engine's whole numbers (volume and pan 0 to 128, delay 0 to 255); these show them as people count.
/** What a sub-column cell reads as, for display and the clipboard. */
export function subText(kind: Exclude<Kind, 'note'>, value: string | null, project: Project): string {
  if (value === null) return '';
  if (kind === 'fx' || kind === 'cmd') return value;
  const n = Number(value);
  if (kind === 'inst') { const index = project.definitions.filter(d => d.kind === 'instr').findIndex(d => d.id === value); return index < 0 ? '??' : String(index).padStart(2, '0'); }
  if (kind === 'vol') return String(Math.round(n * 100 / 128));
  if (kind === 'delay') return String(Math.min(99, Math.round(n * 100 / 256)));
  const pan = Math.round((n - 64) * 50 / 64);
  return pan === 0 ? 'C' : pan > 0 ? `+${pan}` : String(pan);
}
/** The stored value for typed or pasted text, or null when it names nothing. */
export function subValue(kind: Exclude<Kind, 'note'>, text: string, project: Project): string | null {
  if (kind === 'fx') return commandValue(text);
  if (kind === 'cmd') return trackCommandValue(text);
  const t = text.trim().replace(/%$/, '');
  if (kind === 'pan' && /^c$/i.test(t)) return '64';
  if (!/^[+-]?\d{1,3}$/.test(t)) return null;
  const n = Number(t);
  if (kind === 'inst') return n >= 0 && /^\d+$/.test(t) ? project.definitions.filter(d => d.kind === 'instr')[n]?.id ?? null : null;
  if (kind === 'vol') return n >= 0 && n <= 100 && /^\d+$/.test(t) ? String(Math.round(n * 1.28)) : null;
  if (kind === 'delay') return n >= 0 && n <= 99 && /^\d+$/.test(t) ? String(Math.round(n * 2.56)) : null;
  return n >= -50 && n <= 50 ? String(Math.round(64 + n * 64 / 50)) : null;
}

/** The sub-columns that hold what a note does rather than a number to sweep: random and interpolated values would make nonsense of them. */
const expression = (kind: Kind) => kind === 'fx' || kind === 'cmd';

export class PatternGrid {
  projectUndo: ((redo: boolean) => void) | undefined;
  private row = 0;
  private column = 0;
  private anchor = { row: 0, column: 0 };
  private histories = new Map<string, NoteHistory>();
  private targetSignatures = new Map<string, string>();
  private playing: number | null = null;
  private pad = 0;
  /** Set while a blurred cell editor commits, so clicking away does not scroll the grid. */
  private keepView = false;
  /** The cells a click or drag has selected without moving the cursor, outside record mode; null when the selection is the cursor's. */
  private marked: { from: { row: number; column: number }; to: { row: number; column: number } } | null = null;
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

  constructor(
    private parent: HTMLElement,
    private project: () => Project,
    private pattern: () => Pattern,
    private octave: () => number,
    private edited: () => void,
    /** Plays a note: for a moment, or with `hold` (a number for the key) until `releaseHeld` is given it. */
    private audition: (id: string, note: string, track: number, expression?: { commands?: string[]; hold?: number }) => void,
    private togglePlay: () => void,
    private trackSelected: (track: number) => void = () => {},
    private step: () => number = () => 1,
    private feedback: (text: string) => void = () => {},
    private sequence: (action: SequenceAction) => void = () => {},
    /** The instrument that notes entered now are given: the one chosen in the sound browser. */
    private currentInstrument: () => string | null = () => null,
    /** The volume (0 to 100) given to notes entered in record mode, or null to leave it unset. */
    private defaultVelocity: () => number | null = () => null,
  ) {
    document.addEventListener('keyup', event => this.release(event.code));
    window.addEventListener('blur', () => { for (const code of [...this.held.keys()]) this.release(code); });
    this.playhead.className = 'playhead'; this.playhead.hidden = true; this.frame.className = 'track-frame'; this.frame.hidden = true;
    parent.parentElement?.append(this.playhead, this.frame);
    parent.addEventListener('scroll', () => this.placeFrame());
    // the first render can happen before the grid has a size, so centre again once it does
    if (typeof ResizeObserver !== 'undefined') new ResizeObserver(() => { if (parent.clientHeight && this.playing === null && !this.keepView) this.centre(this.row); this.placeFrame(); }).observe(parent);
    // the wheel moves the cursor row like the arrow keys, so the row stays central and the grid scrolls beneath it
    let wheel = 0;
    parent.addEventListener('wheel', event => {
      if (this.live || this.playing !== null || Math.abs(event.deltaY) < Math.abs(event.deltaX) || (event.target as HTMLElement).closest('input')) return;
      event.preventDefault();
      const line = parent.querySelector<HTMLElement>('.grid-row[data-row]')?.offsetHeight || 20;
      wheel += event.deltaMode === 1 ? event.deltaY * line : event.deltaY;
      const rows = Math.trunc(wheel / line); if (!rows) return;
      wheel -= rows * line;
      const row = Math.max(0, Math.min(this.pattern().rows - 1, this.row + rows)); if (row === this.row) return;
      this.row = row; this.anchor = { row, column: this.column }; this.render();
    }, { passive: false });
    parent.addEventListener('pointerdown', event => {
      const cell = (event.target as HTMLElement).closest<HTMLElement>('[data-column]');
      if (!cell || event.button !== 0 || (event.target as HTMLElement).closest('input, .value-drag')) return;
      event.preventDefault(); this.dragging = true;
      const at = { row: Number(cell.dataset.noteRow), column: Number(cell.dataset.column) };
      if (this.recording) {
        this.marked = null; this.row = at.row; this.column = at.column;
        if (!event.shiftKey) this.anchor = { row: this.row, column: this.column };
      } else this.marked = { from: event.shiftKey ? this.marked?.from ?? { row: this.anchor.row, column: this.anchor.column } : at, to: at };
      parent.setPointerCapture?.(event.pointerId); parent.focus(); this.trackSelected(this.activeTrack); this.paintSelection();
    });
    parent.addEventListener('pointermove', event => {
      if (!this.dragging) return;
      const cell = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>('[data-column]');
      if (!cell || !parent.contains(cell)) return;
      const row = Number(cell.dataset.noteRow), column = Number(cell.dataset.column);
      const at = this.marked && !this.recording ? this.marked.to : { row: this.row, column: this.column };
      if (row === at.row && column === at.column) return;
      if (this.marked && !this.recording) this.marked.to = { row, column }; else { this.row = row; this.column = column; }
      this.trackSelected(this.activeTrack); this.paintSelection();
      cell.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    });
    for (const name of ['pointerup', 'pointercancel', 'lostpointercapture']) parent.addEventListener(name, () => { this.dragging = false; });
    // outside record mode a double click is what moves the cursor to the cell
    parent.addEventListener('dblclick', event => {
      const cell = (event.target as HTMLElement).closest<HTMLElement>('[data-column]'); if (!cell || this.recording) return;
      this.row = Number(cell.dataset.noteRow); this.column = Number(cell.dataset.column); this.anchor = { row: this.row, column: this.column }; this.marked = null;
      this.trackSelected(this.activeTrack); this.render();
    });
    parent.addEventListener('keydown', event => this.keydown(event));
    parent.addEventListener('copy', event => this.copy(event));
    parent.addEventListener('cut', event => { if ((event.target as HTMLElement).closest('input')) return; this.copy(event); if (this.recording) this.clearSelection(); });
    parent.addEventListener('paste', event => {
      if ((event.target as HTMLElement).closest('input')) return;
      const rows = parseNotes(event.clipboardData?.getData('text/plain') ?? ''); event.preventDefault();
      if (!rows) { this.feedback('Clipboard needs notes or values, separated by tabs.'); return; }
      // cells marked by a click are only there to copy from: pasting lands at the cursor
      const lanes = this.lanes, edits: NoteEdit[] = [], origin = this.marked ? { row: this.row, column: this.column } : { row: this.bounds.top, column: this.bounds.left };
      for (const [r, values] of rows.entries()) for (const [c, raw] of values.entries()) {
        const row = origin.row + r, column = origin.column + c;
        if (row >= this.pattern().rows || column >= lanes.length) continue;
        let value = raw;
        if (value !== null) {
          const lane = lanes[column];
          if (lane.kind === 'note') value = /^([A-G][#-][0-8]|OFF|CUT)$/.test(value) ? value : null;
          else value = subValue(lane.kind, value, this.project());
          if (value === null) { this.feedback('Paste needs values in the destination lane’s units and range.'); return; }
        }
        edits.push({ row, track: column, before: lanes[column].values[row], after: value });
      }
      this.apply(edits);
    });
  }

  private get lanes(): Lane[] {
    const pattern = this.pattern();
    const project = this.project();
    return Array.from({ length: columnCount(project) }, (_, track): Lane[] => [
      // returns and the master have no notes, only parameter lanes
      ...(track < project.tracks.length ? pattern.notes[track].flatMap((column, col): Lane[] => [
        { track, kind: 'note', col, values: column.notes },
        ...subColumns.filter(kind => kind !== 'fx' && shows(project.tracks[track], kind)).map((kind): Lane => ({ track, kind, col, values: column[kind] ??= [] })),
        ...column.fx.map((values, idx): Lane => ({ track, kind: 'fx', col, values, idx })),
      ]) : []),
      ...columnCommands(project, pattern, track).map((values, idx): Lane => ({ track, kind: 'cmd', col: 0, values, idx })),
    ]).flat();
  }
  private get values() { return this.lanes.map(lane => lane.values); }
  private get barRows() { const t = patternTiming(this.project(), this.pattern()); return Math.max(1, Math.round(t.rows_per_beat * 4 / t.beat_unit * t.beats_per_bar)); }
  get activeTrack() { return this.lanes[this.column]?.track ?? 0; }
  private get history() {
    const pattern = this.pattern(), id = pattern.id;
    const signature = JSON.stringify([[...pattern.fx, ...pattern.return_fx, pattern.master_fx].map(lanes => lanes.map(lane => lane.target)), pattern.notes.map(columns => columns.length), this.project().tracks.map(t => t.show)]);
    if (this.targetSignatures.get(id) !== signature) this.histories.delete(id);
    this.targetSignatures.set(id, signature);
    if (!this.histories.has(id)) this.histories.set(id, new NoteHistory());
    return this.histories.get(id)!;
  }
  resetHistory() { this.histories.clear(); this.anchor = { row: this.row, column: this.column }; }
  undo() { if (this.projectUndo) { this.projectUndo(false); return; } if (this.history.undo(this.values)) { this.render(); this.edited(); } }
  redo() { if (this.projectUndo) { this.projectUndo(true); return; } if (this.history.redo(this.values)) { this.render(); this.edited(); } }
  private apply(edits: NoteEdit[]) {
    if (this.projectUndo) {
      const changed = edits.filter(e => e.before !== e.after);
      if (!changed.length) return;
      const values = this.values;
      for (const edit of changed) values[edit.track][edit.row] = edit.after;
      this.render(); this.edited();
    } else if (this.history.apply(this.values, edits)) { this.render(); this.edited(); }
  }

  selectTrack(track: number, render = true) {
    const index = Math.max(0, Math.min(columnCount(this.project()) - 1, track));
    this.column = this.lanes.findIndex(lane => lane.track === index);
    this.anchor = { row: this.row, column: this.column }; this.trackSelected(index);
    if (render) this.render();
    else { this.paintSelection(); for (const header of this.parent.querySelectorAll<HTMLElement>('.track-header')) header.classList.toggle('selected-track', Number(header.dataset.track) === index); }
  }
  private id(row: number, lane: Lane) {
    return lane.kind === 'note' && lane.col === 0 ? `cell-${row}-${lane.track}` : `cell-${row}-${lane.track}-${lane.col}-${lane.kind}${lane.idx ? lane.idx + 1 : ''}`;
  }
  private text(lane: Lane, value: string | null) { return lane.kind === 'note' ? value ?? '' : subText(lane.kind, value, this.project()); }
  /** The width of a column: a track's note columns with their sub-columns, then its command columns. */
  private trackWidth(track: number) {
    const lanes = columnCommands(this.project(), this.pattern(), track).length * FX_WIDTH;
    if (track >= this.project().tracks.length) return Math.max(MIN_STRIP_WIDTH, lanes + 2);
    const subs = subColumns.filter(kind => kind !== 'fx' && shows(this.project().tracks[track], kind)).length;
    return Math.max(MIN_TRACK_WIDTH, this.pattern().notes[track].reduce((width, column) => width + NOTE_WIDTH + subs * SUB_WIDTH + column.fx.length * FX_WIDTH, 0) + 2 + lanes);
  }
  private selected(row: number, column: number) {
    const { top, bottom, left, right } = this.bounds;
    return row >= top && row <= bottom && column >= left && column <= right;
  }
  private paintSelection() {
    for (const cell of this.parent.querySelectorAll<HTMLElement>('[data-column]')) {
      const row = Number(cell.dataset.noteRow), column = Number(cell.dataset.column), selected = this.selected(row, column);
      cell.classList.toggle('range-selected', selected); cell.setAttribute('aria-selected', String(selected));
      cell.classList.toggle('cursor', row === this.row && column === this.column);
    }
    for (const line of this.parent.querySelectorAll<HTMLElement>('.grid-row[data-row]')) line.classList.toggle('cursor-row', Number(line.dataset.row) === this.row);
    for (const header of this.parent.querySelectorAll<HTMLElement>('[data-track]')) header.classList.toggle('selected-track', Number(header.dataset.track) === this.activeTrack);
    this.parent.setAttribute('aria-activedescendant', this.id(this.row, this.lanes[this.column])); this.placeFrame();
  }

  /** `recentre` scrolls the cursor row to the middle; a click passes false so the grid stays put under the pointer. */
  render(recentre = true) {
    const project = this.project(), pattern = this.pattern(), lanes = this.lanes;
    const timing = patternTiming(project, pattern), beatRows = timing.rows_per_beat * 4 / timing.beat_unit;
    this.column = Math.min(this.column, lanes.length - 1); this.anchor.column = Math.min(this.anchor.column, lanes.length - 1);
    this.row = Math.min(this.row, pattern.rows - 1); this.anchor.row = Math.min(this.anchor.row, pattern.rows - 1);
    // a button of a header that is about to be redrawn must not take the keyboard with it
    const refocus = document.activeElement !== this.parent && this.parent.contains(document.activeElement);
    this.parent.replaceChildren();
    if (refocus) this.parent.focus({ preventScroll: true });
    this.parent.style.setProperty('--track-columns', `${ROW_WIDTH}px ${Array.from({ length: columnCount(project) }, (_, track) => `${this.trackWidth(track)}px`).join(' ')}`);
    const header = document.createElement('div'); header.className = 'grid-row grid-header'; header.setAttribute('role', 'row');
    const label = document.createElement('span'); label.setAttribute('role', 'columnheader'); header.append(label);
    for (let index = 0; index < columnCount(project); index++) {
      // a track, or a return, or the master after them: the last two have parameter lanes but no notes
      const track = columnStrip(project, index), isTrack = index < project.tracks.length, isMaster = index === columnCount(project) - 1;
      const cell = document.createElement('div'); cell.setAttribute('role', 'columnheader'); cell.className = isTrack ? 'track-header' : 'track-header strip'; cell.dataset.track = String(index);
      const slot = isTrack ? this.slot(index) : null; if (slot) { cell.dataset.slot = slot.kind; cell.title = slot.text; }
      cell.style.setProperty('--track-colour', columnColour(project, index)); cell.classList.toggle('selected-track', index === this.activeTrack);
      const strip = document.createElement(isTrack ? 'button' : 'div'); strip.className = 'track-colour';
      if (isTrack) { strip.title = 'Track colour'; strip.setAttribute('aria-label', `${track.name} colour`); strip.onclick = () => this.pickColour(track, strip); }
      const heading = document.createElement('div'); heading.className = 'track-heading';
      const name = document.createElement('button'); name.className = 'track-name'; name.textContent = track.name; name.title = isMaster ? 'The master: everything is mixed into it' : 'Double-click to rename'; name.onclick = () => this.selectTrack(index, false);
      if (!isMaster) name.ondblclick = () => {
        const input = document.createElement('input'); input.className = 'track-rename'; input.value = track.name; input.maxLength = 64; input.setAttribute('aria-label', isTrack ? `Rename track ${index + 1}` : `Rename ${track.name}`);
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
      const mute = document.createElement('button'); mute.textContent = 'M'; mute.title = `Mute ${track.name}`; mute.setAttribute('aria-label', mute.title); mute.setAttribute('aria-pressed', String(track.muted));
      mute.onclick = () => { track.muted = !track.muted; this.edited(); this.render(); };
      const solo = document.createElement('button'); solo.textContent = 'S'; solo.title = `Solo ${track.name}`; solo.setAttribute('aria-label', solo.title); solo.setAttribute('aria-pressed', String(!!track.solo));
      solo.onclick = () => { track.solo = !track.solo; this.edited(); this.render(); };
      if (!isMaster) tools.append(mute); if (isTrack) tools.append(solo);
      const more = document.createElement('button'); more.textContent = '⋯'; more.title = isTrack ? `${track.name} note columns and commands` : `${track.name} commands`; more.setAttribute('aria-label', `${track.name} columns`);
      more.className = 'track-more'; more.onclick = () => { this.selectTrack(index, false); this.pickColumns(index, more); }; tools.append(more);
      // a dot after the name marks a track that has effects
      const effects = track.fx.length ? [Object.assign(document.createElement('span'), { className: 'has-fx', textContent: '●', title: `${track.fx.length} effect${track.fx.length > 1 ? 's' : ''}` })] : [];
      heading.append(name, ...effects);
      const columns = document.createElement('div'); columns.className = 'track-columns';
      for (const lane of lanes.filter(lane => lane.track === index)) {
        const label = document.createElement('span'); label.textContent = subLabel[lane.kind];
        label.className = lane.kind === 'note' ? 'col-label' : lane.kind === 'fx' || lane.kind === 'cmd' ? 'col-label sub-label fx-label' : 'col-label sub-label'; if (lane.kind === 'note' && lane.col > 0) label.classList.add('col-start');
        if (lane.kind !== 'note') label.title = subTitle[lane.kind]; columns.append(label);
      }
      cell.append(strip, heading, tools, columns); cell.onkeydown = event => event.stopPropagation();
      cell.onclick = event => { if (!(event.target as HTMLElement).closest('button, select, input')) this.selectTrack(index, false); };
      header.append(cell);
    }
    this.parent.append(header, this.pads());
    for (let row = 0; row < pattern.rows; row++) {
      const line = document.createElement('div'); line.className = `grid-row ${row % (beatRows) === 0 ? 'beat' : ''} ${row % (beatRows * timing.beats_per_bar) === 0 ? 'bar' : ''}`; line.setAttribute('role', 'row'); line.dataset.row = String(row);
      const number = document.createElement('span'); number.className = 'row-number'; number.textContent = String(row).padStart(2, '0'); line.append(number);
      let column = 0;
      for (let track = 0; track < columnCount(project); track++) {
        const group = document.createElement('div'); group.className = 'track-cells';
        for (const lane of lanes.filter(lane => lane.track === track)) {
          const c = column++, value = lane.values[row], cell = document.createElement('div'); cell.setAttribute('role', 'gridcell');
          cell.className = `${lane.kind === 'note' ? 'note' : lane.kind === 'fx' || lane.kind === 'cmd' ? 'sub fxc' : 'sub'} ${value ? 'filled' : ''}`; cell.dataset.noteRow = String(row); cell.dataset.column = String(c);
          if (lane.kind === 'note' && lane.col > 0) cell.classList.add('col-start'); if (value === 'OFF') cell.classList.add('off'); if (value === 'CUT') cell.classList.add('cut');
          cell.id = this.id(row, lane); cell.textContent = value === null ? lane.kind === 'note' ? '···' : '··' : this.text(lane, value);
          if (value !== null && (lane.kind === 'fx' || lane.kind === 'cmd')) cell.title = `${value} ${this.explain(lane.kind, value, lane.track)}`;
          cell.onclick = event => {
            if (!this.recording) return;
            if (!event.shiftKey) this.anchor = { row, column: c };
            this.row = row; this.column = c; this.trackSelected(track); this.render(); this.parent.focus();
            if (!event.shiftKey && lane.kind !== 'note') this.editSub(row, c);
          };
          group.append(cell);
        }
        line.append(group);
      }
      this.parent.append(line);
    }
    this.parent.append(this.pads());
    this.paintSelection();
    const cursor = this.parent.querySelector<HTMLElement>(`#${this.id(this.row, lanes[this.column])}`);
    if (recentre && !this.keepView && this.playing === null) this.centre(this.row);
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
    if (row === null) { this.playhead.hidden = true; this.shown = -1; if (was !== null) this.placeFrame(); return; }
    if (was === null) this.placeFrame();
    const t = patternTiming(this.project(), this.pattern());
    const swing = patternSwing(this.project(), this.pattern()), rows = this.pattern().rows, start = patternSeconds(t, row, swing, rows), end = patternSeconds(t, row+1, swing, rows);
    this.origin = atMs - (start + (phase ?? 0) * (end-start)) * 1000;
    this.animation ||= requestAnimationFrame(this.follow);
  }
  /** Recording while the pattern plays: the cursor rides the play position and notes land on the nearest row. */
  private get live() { return this.recording && this.playing !== null; }
  private currentRow() { return patternRow(patternTiming(this.project(), this.pattern()), (Date.now() - this.origin) / 1000, this.pattern().rows, patternSwing(this.project(), this.pattern())); }
  private nearestRow() { return Math.round(this.currentRow()) % this.pattern().rows; }
  private follow = () => {
    this.animation = 0;
    if (this.playing === null) return;
    const row = Math.min(Math.max(Math.floor(this.currentRow()), 0), this.pattern().rows - 1);
    if (row !== this.shown) {
      this.shown = row; this.followRow(row);
      if (this.live) { this.row = row; this.anchor = { row, column: this.column }; this.paintSelection(); }
    }
    this.animation = requestAnimationFrame(this.follow);
  };
  /**
   * While playing, the pattern steps up one row at a time under a play bar that stays near the middle.
   * Padding above the first row and below the last lets the bar stay put at both ends.
   */
  private followRow(row: number) {
    const top = this.centre(row); if (top === null) return;
    const header = this.parent.querySelector<HTMLElement>('.grid-header')?.offsetHeight ?? 0;
    const y = top - this.parent.scrollTop;
    this.playhead.hidden = y < header || y > this.parent.clientHeight;
    this.playhead.style.top = `${y}px`;
  }
  /** A small panel under `anchor` that closes on a click elsewhere or Escape. */
  private popup(anchor: HTMLElement, className: string, focusGrid = false) {
    document.querySelector('.colour-popover')?.remove();
    const element = document.createElement('div'); element.className = `colour-popover ${className}`;
    const outside = (event: Event) => { if (!element.contains(event.target as Node)) close(); };
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') { event.stopPropagation(); close(); if (focusGrid) this.parent.focus(); } };
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
    const project = this.project(), track = columnStrip(project, index), isTrack = index < project.tracks.length;
    const changed = () => {
      this.resetHistory(); this.edited(); this.render(); close();
      const again = this.parent.querySelector<HTMLElement>(`.track-header[data-track="${index}"] .track-more`); if (again) this.pickColumns(index, again);
    };
    const count = isTrack ? this.pattern().notes[index].length : 0;
    const heading = document.createElement('span'); heading.textContent = 'Note columns';
    const fewer = document.createElement('button'); fewer.textContent = '−'; fewer.setAttribute('aria-label', 'Fewer note columns'); fewer.disabled = count <= 1;
    fewer.onclick = () => {
      if (project.patterns.some(p => { const last = p.notes[index].at(-1)!; return [last.notes, last.inst, last.vol, last.pan, last.delay, ...last.fx].some(cells => cells.some(v => v !== null)); })) { this.feedback('Clear the last note column in every pattern before removing it.'); return; }
      for (const p of project.patterns) p.notes[index].pop();
      changed();
    };
    const amount = document.createElement('strong'); amount.textContent = String(count);
    const more = document.createElement('button'); more.textContent = '+'; more.setAttribute('aria-label', 'More note columns'); more.disabled = count >= maxNoteColumns;
    more.onclick = () => { for (const p of project.patterns) p.notes[index].push(blankColumn(Math.max(p.rows, p.notes[index][0].notes.length), p.notes[index][0].fx.length)); changed(); };
    // a return or the master has no notes, so only a track has note columns and sub-columns to set
    const line = (...nodes: Node[]) => { const div = document.createElement('div'); div.className = 'choice-row'; div.append(...nodes); return div; };
    const section = (title: string, hint: string) => {
      const box = document.createElement('div'); box.className = 'choice-section';
      const name = document.createElement('strong'); name.textContent = title; const text = document.createElement('small'); text.textContent = hint; box.append(name, text); return box;
    };
    heading.textContent = 'Count';
    if (isTrack) element.append(section('Note columns', 'More than one lets a track play chords or overlapping notes.'), line(heading, fewer, amount, more));
    if (isTrack) {
      const fxCount = this.pattern().notes[index][0].fx.length, rowsOf = (p: Pattern, cells: unknown[]) => Math.max(p.rows, cells.length);
      const fewerFx = document.createElement('button'); fewerFx.textContent = '−'; fewerFx.setAttribute('aria-label', 'Fewer command columns'); fewerFx.disabled = fxCount < 1;
      fewerFx.onclick = () => {
        if (project.patterns.some(p => p.notes[index].some(column => column.fx.at(-1)?.some(v => v !== null)))) { this.feedback('Clear the last command column before hiding it.'); return; }
        for (const p of project.patterns) for (const column of p.notes[index]) column.fx.pop();
        changed();
      };
      const fxAmount = document.createElement('strong'); fxAmount.textContent = String(fxCount);
      const moreFx = document.createElement('button'); moreFx.textContent = '+'; moreFx.setAttribute('aria-label', 'More command columns'); moreFx.disabled = fxCount >= maxFxColumns;
      moreFx.onclick = () => { for (const p of project.patterns) for (const column of p.notes[index]) column.fx.push(Array(rowsOf(p, column.notes)).fill(null)); changed(); };
      const fxName = document.createElement('span'); fxName.textContent = 'Count';
      element.append(section('Command columns', 'On a row: S20 starts the sample partway in, G10 glides, R03 retriggers; L80 sets the track volume, P80 its pan, 12FF parameter 2 of effect 1. F1 lists them.'), line(fxName, fewerFx, fxAmount, moreFx));
      element.append(section('Extra columns for each note', 'Hover a name for what it does.'));
    }
    const checks = document.createElement('div'); checks.className = 'choice-checks'; if (isTrack) element.append(checks);
    for (const kind of isTrack ? subColumns.filter(kind => kind !== 'inst' && kind !== 'fx') : []) {
      const label = document.createElement('label'); label.title = subTitle[kind];
      const box = document.createElement('input'); box.type = 'checkbox'; box.checked = !!track.show?.[kind]; box.setAttribute('aria-label', `Show ${subLabel[kind]} column`);
      box.onchange = () => { track.show = { ...track.show, [kind]: box.checked }; changed(); };
      label.append(box, { vol: 'Velocity', pan: 'Panning', delay: 'Delay' }[kind as 'vol' | 'pan' | 'delay']); checks.append(label);
    }
    if (!isTrack) {
      const at = index - project.tracks.length, commandCount = columnCommands(project, this.pattern(), index).length;
      const fewerCommands = document.createElement('button'); fewerCommands.textContent = '−'; fewerCommands.setAttribute('aria-label', 'Fewer command columns'); fewerCommands.disabled = commandCount <= 1; // it keeps one for the cursor
      fewerCommands.onclick = () => {
        if (project.patterns.some(p => p.commands[at].at(-1)?.some(v => v !== null))) { this.feedback('Clear the last command column before hiding it.'); return; }
        for (const p of project.patterns) p.commands[at].pop();
        changed();
      };
      const commandAmount = document.createElement('strong'); commandAmount.textContent = String(commandCount);
      const moreCommands = document.createElement('button'); moreCommands.textContent = '+'; moreCommands.setAttribute('aria-label', 'More command columns'); moreCommands.disabled = commandCount >= maxTrackCommandColumns;
      moreCommands.onclick = () => { for (const p of project.patterns) p.commands[at].push(Array(p.rows).fill(null)); changed(); };
      const commandName = document.createElement('span'); commandName.textContent = 'Count';
      element.append(section('Command columns', 'On a row: L80 sets the volume, P80 the pan, 12FF parameter 2 of effect 1. Draw automation in the Automation tab.'), line(commandName, fewerCommands, commandAmount, moreCommands));
    }
  }
  /** A command in words, for its cell's tooltip and the status line. */
  private explain(kind: 'fx' | 'cmd', text: string, track: number) {
    return describeAnyCommand(text, this.project(), columnStrip(this.project(), track));
  }
  /** Edits a sub-column cell in place, like a parameter lane: type a number, Enter to save and move down. */
  private editSub(row: number, column: number, initial?: string) {
    const lane = this.lanes[column], kind = lane.kind as Exclude<Kind, 'note'>, cell = this.parent.querySelector<HTMLElement>(`#${this.id(row, lane)}`);
    if (!cell) return;
    const input = document.createElement('input'); input.className = 'lane-value'; input.setAttribute('aria-label', `${subLabel[kind]} row ${row}`);
    input.value = initial ?? this.text(lane, lane.values[row]); if (kind === 'fx' || kind === 'cmd') input.maxLength = 4; const originalText = input.value;
    cell.replaceChildren(input); input.focus(); if (initial === undefined) input.select();
    let finished = false;
    const finish = (commit: boolean, advance: boolean) => {
      if (finished) return;
      const blank = input.value.trim() === '';
      const value = initial === undefined && input.value === originalText ? lane.values[row] : blank ? null : subValue(kind, input.value, this.project());
      const notes = this.pattern().notes[lane.track]?.[lane.col];
      const competing = commit && value !== null && kind === 'fx' && notes && glideRetriggers(notes.fx.map((cells, i) => i === lane.idx ? value : cells[row]));
      if (competing) { this.feedback('A glide and a retrigger cannot share a row: clear one first.'); if (advance) { input.focus(); return; } commit = false; }
      if (commit && !blank && value === null) { this.feedback(kind === 'inst' ? 'No instrument has that number.' : kind === 'fx' ? commandProblem(input.value.trim().toUpperCase()) ?? 'Not a command.' : kind === 'cmd' ? trackCommandProblem(input.value.trim().toUpperCase()) ?? 'Not a track command.' : `Enter ${subRange[kind]}.`); if (advance) { input.focus(); return; } commit = false; }
      finished = true; this.keepView = !advance;
      if (commit) this.apply([{ row, track: column, before: lane.values[row], after: value }]);
      if (commit && value && (kind === 'fx' || kind === 'cmd')) this.feedback(`${value} ${this.explain(kind, value, lane.track)}`);
      if (commit && kind === 'fx' && value && 'SBR'.includes(value[0])) this.previewCommand(row, lane, value);
      if (advance && commit) this.advance(); this.render(); this.keepView = false; if (advance) this.parent.focus();
    };
    input.onkeydown = event => { event.stopPropagation(); if (event.key === 'Enter' || event.key === 'Escape' || event.key === 'Tab') { event.preventDefault(); finish(event.key !== 'Escape', true); } };
    if (kind === 'fx' || kind === 'cmd') {
      const explainTyped = () => { const text = input.value.trim().toUpperCase(); if ((kind === 'fx' ? commandProblem(text) : trackCommandProblem(text)) === null) this.feedback(`${text} ${this.explain(kind, text, lane.track)}`); };
      input.oninput = explainTyped;
    }
    input.onblur = () => finish(true, false);
  }
  /** Plays the note a command at `row` acts on, with the command: the note in the row, or the one held above it. */
  private previewCommand(row: number, lane: Lane, value: string) {
    const column = this.pattern().notes[lane.track]?.[lane.col]; if (!column) return;
    for (let at = row; at >= 0; at--) {
      const note = column.notes[at];
      if (note === null) continue;
      if (noteNumber(note) === null) return;
      const instrument = instrumentAt(column.inst, at);
      if (instrument) this.audition(instrument, note, lane.track, { commands: [value] });
      return;
    }
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
    this.frame.style.cssText = `left:${left}px;width:${right - left}px;top:0;height:${Math.max(0, bottom)}px;--c:${columnColour(this.project(), this.activeTrack)};--side:${header.offsetHeight}px`;
  }
  /** Scrolls `row` to a third of the way down the view, as Renoise does; the padding below lets the last row get there too. */
  private centre(row: number) {
    const header = this.parent.querySelector<HTMLElement>('.grid-header')?.offsetHeight ?? 0;
    const body = this.parent.clientHeight - header, above = Math.round(body / 3), pad = body - above; if (pad !== this.pad) this.setPad(pad);
    const top = this.contentTop(row); if (top === null) return null;
    const half = (this.parent.querySelector<HTMLElement>(`[data-row="${row}"]`)?.offsetHeight ?? 0) / 2;
    this.parent.scrollTop = top + half - header - above;
    return top;
  }
  private setPad(pad: number) { this.pad = pad; for (const spacer of this.parent.querySelectorAll<HTMLElement>('.grid-pad')) spacer.style.height = `${pad}px`; }
  private pads() { const spacer = document.createElement('div'); spacer.className = 'grid-pad'; spacer.style.height = `${this.pad}px`; return spacer; }
  private contentTop(row: number) {
    const line = this.parent.querySelector<HTMLElement>(`[data-row="${row}"]`); if (!line) return null;
    return line.getBoundingClientRect().top - this.parent.getBoundingClientRect().top + this.parent.scrollTop;
  }
  private copy(event: ClipboardEvent) {
    if ((event.target as HTMLElement).closest('input')) return;
    event.clipboardData?.setData('text/plain', this.selectionText()); event.preventDefault();
  }
  /** The selection as the clipboard holds it: a line for each row, the cells separated by tabs. */
  private selectionText() {
    const rows: string[] = [], lanes = this.lanes;
    const { top, bottom, left, right } = this.bounds;
    for (let row = top; row <= bottom; row++) {
      const values: string[] = [];
      for (let c = left; c <= right; c++) {
        const lane = lanes[c], value = lane.values[row];
        values.push(value === null ? '---' : this.text(lane, String(value)));
      }
      rows.push(values.join('\t'));
    }
    return rows.join('\n');
  }
  /** Copies the whole track the cursor is in and clears it (clearing needs record mode, as every edit does). */
  cutTrack() {
    this.selectWholeTrack();
    void navigator.clipboard?.writeText(this.selectionText()).catch(() => this.feedback('Could not reach the clipboard.'));
    if (this.recording) this.clearSelection(); else this.feedback('Copied the track. Turn on record mode to clear it.');
  }
  /** Reverses the order of the selected rows in each selected column. */
  reverseSelection() {
    if (!this.recording) { this.feedback('Turn on record mode to edit the selection.'); return; }
    const { top, bottom, left, right } = this.bounds, edits: NoteEdit[] = [];
    for (let column = left; column <= right; column++) {
      const lane = this.lanes[column]; 
      for (let row = top; row <= bottom; row++) edits.push({ row, track: column, before: lane.values[row], after: lane.values[top + bottom - row] });
    }
    this.apply(edits);
  }
  /** Random generator for {@link randomizeSelection}; replaced in tests. */
  random: () => number = Math.random;
  /** Gives each note or value in the selection a new random one between the lowest and highest in its column; blanks, OFF and CUT stay. */
  randomizeSelection() {
    if (!this.recording) { this.feedback('Turn on record mode to edit the selection.'); return; }
    const { top, bottom, left, right } = this.bounds, edits: NoteEdit[] = [];
    for (let column = left; column <= right; column++) {
      const lane = this.lanes[column]; if (lane.kind === 'inst' || expression(lane.kind)) continue;
      const number = (value: string | null) => value === null ? null : lane.kind === 'note' ? noteNumber(value) : Number.isFinite(Number(value)) ? Number(value) : null;
      const rows = Array.from({ length: bottom - top + 1 }, (_, i) => top + i).filter(row => number(lane.values[row]) !== null);
      const numbers = rows.map(row => number(lane.values[row])!), low = Math.min(...numbers), high = Math.max(...numbers);
      if (rows.length < 2 || low === high) continue;
      for (const row of rows) {
        const pick = low + this.random() * (high - low);
        const after = lane.kind === 'note' ? noteAt(Math.round(pick)) : String(Math.round(pick));
        edits.push({ row, track: column, before: lane.values[row], after });
      }
    }
    if (!edits.length) { this.feedback('Randomize needs two or more notes or values in a column, that differ.'); return; }
    this.apply(edits);
  }
  /** Selects every row of every column of the track the cursor is in, ready to copy or cut and paste onto another track or pattern. */
  selectWholeTrack() {
    const columns = this.lanes.flatMap((lane, index) => lane.track === this.activeTrack ? [index] : []);
    this.anchor = { row: 0, column: columns[0] }; this.row = this.pattern().rows - 1; this.column = columns[columns.length - 1];
    this.trackSelected(this.activeTrack); this.render();
  }
  /** The selected rectangle: first and last row and column. */
  private get bounds() {
    const a = this.marked?.from ?? this.anchor, b = this.marked?.to ?? { row: this.row, column: this.column };
    return { top: Math.min(a.row, b.row), bottom: Math.max(a.row, b.row), left: Math.min(a.column, b.column), right: Math.max(a.column, b.column) };
  }
  /** Moves the selected notes up or down by `semitones`; OFF, CUT and notes that would leave the range stay as they are. */
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
      if (lane.kind === 'note' || lane.kind === 'inst' || expression(lane.kind)) continue;
      const from = Number(lane.values[top]), to = Number(lane.values[bottom]);
      if (lane.values[top] === null || lane.values[bottom] === null || !Number.isFinite(from) || !Number.isFinite(to)) continue;
      for (let row = top + 1; row < bottom; row++) {
        const value = from + (to - from) * (row - top) / (bottom - top);
        edits.push({ row, track: column, before: lane.values[row], after: String(Math.round(value)) });
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
      if ((scope === 'track' ? !tracks.has(lane.track) : column < left || column > right)) return;
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
      for (let row = top; row <= bottom && row + height < rows; row++) edits.push({ row: row + height, track: column, before: lanes[column].values[row + height], after: lanes[column].values[row] });
    }
    this.apply(edits);
    const shift = Math.min(height, rows - 1 - bottom); this.row += shift; this.anchor.row += shift;
    this.render();
  }
  private clearSelection() {
    const edits: NoteEdit[] = [];
    this.lanes.forEach((lane, column) => { for (let row = 0; row < this.pattern().rows; row++) if (this.selected(row, column)) edits.push({ row, track: column, before: lane.values[row], after: null }); }); this.apply(edits);
  }
  private advance() { this.row = (this.row + Math.max(0, Math.min(256, Math.trunc(this.step()) || 0))) % this.pattern().rows; this.anchor = { row: this.row, column: this.column }; }
  /**
   * Records a moved control as automation: a point on the envelope of the lane for `target` (in the track's first free lane if it has
   * none yet) at the cursor's row, switching the envelope on.
   */
  writeParameter(track: number, target: FxTarget, value: number) {
    const pattern = this.pattern(), lanes = columnLanes(this.project(), pattern, track);
    let lane = lanes.find(other => targetKey(other.target) === targetKey(target));
    if (!lane) {
      lane = lanes.find(other => other.target === null && !other.envelope?.points.length);
      if (!lane && lanes.length < maxLanes) { lane = { target: null }; lanes.push(lane); }
      if (!lane) { this.feedback('Remove an automation lane before recording this control (maximum 16 per track).'); return; }
      lane.target = { ...target };
    }
    const envelope = lane.envelope ??= { enabled: true, mode: 'hold', points: [] };
    envelope.enabled = true;
    const row = this.row, point = envelope.points.find(p => p.row === row);
    if (point) point.value = value; else { envelope.points.push({ row, value }); envelope.points.sort((a, b) => a.row - b.row); }
    this.resetHistory(); this.edited(); this.advance(); this.render(); this.parent.focus();
    this.feedback(`Recorded at row ${row} of this pattern: see the Automation tab`);
  }
  /** MIDI uses absolute pitch and hardware velocity, even when the cursor is on a sub-column or lane. */
  midiNoteOn(number: number, velocity: number) {
    if (!Number.isInteger(number) || number < 12 || number > 119 || !Number.isInteger(velocity) || velocity < 1 || velocity > 127) return null;
    const note = transposeNote('C-4', number - 60)!;
    const selected = this.lanes[this.column];
    const track = Math.min(selected.track, this.project().tracks.length - 1);
    const col = selected.track === track ? selected.col : 0;
    const noteLane = this.lanes.findIndex(l => l.track === track && l.col === col && l.kind === 'note');
    const column = this.pattern().notes[track][col];
    const row = this.live ? this.nearestRow() : this.row;
    const instrument = this.currentInstrument() ?? instrumentAt(column.inst, row);
    if (this.recording) {
      this.column = noteLane;
      this.writeNote(note, velocity / 127 * 100, false);
    }
    return { note, track, instrument, velocity };
  }

  private writeNote(note: string, velocity: number | null, preview = true, key?: KeyboardEvent) {
    const lane = this.lanes[this.column], ending = note === 'OFF' || note === 'CUT';
    const live = this.live; if (live) { this.row = this.nearestRow(); this.anchor = { row: this.row, column: this.column }; }
    const column = this.pattern().notes[lane.track][lane.col], inst = ending ? undefined : this.currentInstrument();
    const instrument = !ending && (inst ?? instrumentAt(column.inst, this.row));
    if (instrument && preview) { if (key) this.hold(key, instrument, note, lane.track); else this.audition(instrument, note, lane.track); }
    const track = this.project().tracks[lane.track];
    if (velocity !== null && !track.show?.vol) track.show = { ...track.show, vol: true };
    this.column = this.lanes.findIndex(other => other.track === lane.track && other.col === lane.col && other.kind === 'note');
    const instLane = this.lanes.findIndex(other => other.track === lane.track && other.col === lane.col && other.kind === 'inst');
    const volLane = this.lanes.findIndex(other => other.track === lane.track && other.col === lane.col && other.kind === 'vol');
    this.apply([{ row: this.row, track: this.column, before: lane.values[this.row], after: note }, ...(inst ? [{ row: this.row, track: instLane, before: column.inst[this.row], after: inst }] : []), ...(velocity !== null && volLane >= 0 ? [{ row: this.row, track: volLane, before: column.vol[this.row], after: String(Math.round(velocity * 1.28)) }] : [])]);
    if (!live) this.advance();
    this.render();
  }

  /** Called when a note started by a key is to stop, because the key came up: the number it was started with. */
  releaseHeld: (key: number) => void = () => {};
  /** The notes sounding for keys held down, by physical key, with the number each was started with. */
  private readonly held = new Map<string, number>();
  /** Plays a note for as long as its key is held. A key that repeats does not start it again. */
  private hold(event: KeyboardEvent, instrument: string, note: string, track: number) {
    if (event.repeat) return;
    const number = noteNumber(note);
    if (number === null) { this.audition(instrument, note, track); return; }
    const key = 2048 + number;
    this.release(event.code);
    this.held.set(event.code, key);
    this.audition(instrument, note, track, { hold: key });
  }
  private release(code: string) {
    const key = this.held.get(code);
    if (key === undefined) return;
    this.held.delete(code);
    this.releaseHeld(key);
  }

  /** Plays the key as a note on the cursor's track while focus is outside the grid, so the keyboard sounds from anywhere. */
  auditionKey(event: KeyboardEvent) {
    if (event.ctrlKey || event.metaKey || event.altKey || event.repeat) return;
    const note = keyboardNote(event.key, this.octave()), instrument = this.currentInstrument();
    if (!note || !instrument) return;
    event.preventDefault();
    this.hold(event, instrument, note, Math.min(this.lanes[this.column].track, this.project().tracks.length - 1));
  }
  private keydown(event: KeyboardEvent) {
    if (this.marked && !event.metaKey && !event.ctrlKey) this.marked = null;
    if ((event.target as HTMLElement).closest('input, select, textarea')) return;
    if (event.ctrlKey || event.metaKey) {
      const key = event.key.length === 1 ? event.key.toLowerCase() : event.key;
      // Option+arrows alone choose the instrument, and Cmd+arrows alone move through the sequence, so
      // transposing is Cmd/Ctrl+Option+arrows for a semitone and Cmd/Ctrl+Shift+arrows for an octave
      if ((key === 'ArrowUp' || key === 'ArrowDown') && (event.altKey || event.shiftKey)) {
        event.preventDefault();
        if (this.recording && event.altKey !== event.shiftKey) this.transpose((key === 'ArrowUp' ? 1 : -1) * (event.shiftKey ? 12 : 1));
        return;
      }
      if (key === 'z') { event.preventDefault(); if (event.shiftKey) this.redo(); else this.undo(); }
      if (key === 'y') { event.preventDefault(); this.redo(); }
      if (key === 'a') { event.preventDefault(); this.marked = null; if (event.shiftKey) this.selectWholeTrack(); else { this.anchor = { row: 0, column: 0 }; this.row = this.pattern().rows - 1; this.column = this.lanes.length - 1; this.trackSelected(this.activeTrack); this.render(); } }
      const sequence: Record<string, SequenceAction> = { ArrowUp: 'previous', ArrowDown: 'next', Home: 'first', End: 'last', ArrowLeft: 'pattern-previous', ArrowRight: 'pattern-next', ...(event.shiftKey ? {} : { Insert: 'new', Delete: 'remove' }) };
      const edit: Record<string, () => void> = { i: () => this.interpolate(), d: () => this.duplicate(), Enter: () => this.shiftRows(1), Backspace: () => this.shiftRows(-1), ...(event.shiftKey ? { Insert: () => this.shiftRows(1, 'column'), Delete: () => this.shiftRows(-1, 'column') } : {}) };
      if (sequence[key]) { event.preventDefault(); this.sequence(sequence[key]); }
      else if (edit[key]) { event.preventDefault(); if (this.recording) edit[key](); }
      return;
    }
    if (event.altKey) return;
    const lanes = this.lanes, lane = lanes[this.column];
    // the play position owns the cursor's row while recording over playback
    if (this.live && ['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', 'F9', 'F10', 'F11', 'F12'].includes(event.key)) { event.preventDefault(); return; }
    switch (event.key) {
      case ' ': event.preventDefault(); this.togglePlay(); return;
      case 'ArrowUp': this.row = event.shiftKey ? Math.max(0, this.row - 1) : (this.row + this.pattern().rows - 1) % this.pattern().rows; break;
      case 'ArrowDown': this.row = event.shiftKey ? Math.min(this.pattern().rows - 1, this.row + 1) : (this.row + 1) % this.pattern().rows; break;
      case 'ArrowLeft': this.column = event.shiftKey ? Math.max(0, this.column - 1) : (this.column + lanes.length - 1) % lanes.length; break;
      case 'ArrowRight': this.column = event.shiftKey ? Math.min(lanes.length - 1, this.column + 1) : (this.column + 1) % lanes.length; break;
      case 'Tab': this.selectTrack((this.activeTrack + (event.shiftKey ? columnCount(this.project()) - 1 : 1)) % columnCount(this.project())); event.preventDefault(); return;
      case 'PageUp': this.row = Math.max(0, this.row - this.barRows); break;
      case 'PageDown': this.row = Math.min(this.pattern().rows - 1, this.row + this.barRows); break;
      case 'Home': case 'F9': this.row = 0; break;
      case 'End': this.row = this.pattern().rows - 1; break;
      case 'F10': this.row = Math.floor(this.pattern().rows / 4); break;
      case 'F11': this.row = Math.floor(this.pattern().rows / 2); break;
      case 'F12': this.row = Math.floor(this.pattern().rows * 3 / 4); break;
      case 'Insert': if (!this.recording) return; event.preventDefault(); this.shiftRows(1); return;
      case 'Backspace': if (!this.recording) return; event.preventDefault(); this.shiftRows(-1); return;
      case 'Delete': if (!this.recording) return; event.preventDefault(); this.clearSelection(); return;
      default: {
        if (!this.recording) {
          const note = keyboardNote(event.key, this.octave());
          if (!note) return;
          event.preventDefault();
          const track = Math.min(lane.track, this.project().tracks.length - 1);
          const instrument = this.currentInstrument() ?? instrumentAt(this.pattern().notes[track]?.[lane.col]?.inst ?? [], this.row);
          if (instrument) this.hold(event, instrument, note, track);
          return;
        }
        if (lane.kind !== 'note') {
          const typed = lane.kind === 'fx' || lane.kind === 'cmd' ? /^[a-z0-9]$/i.test(event.key) : /^[0-9+-]$/.test(event.key) || lane.kind === 'pan' && /^c$/i.test(event.key);
          if (!(event.key === 'Enter' || typed)) return;
          event.preventDefault(); this.editSub(this.row, this.column, event.key === 'Enter' ? undefined : lane.kind === 'fx' || lane.kind === 'cmd' ? event.key.toUpperCase() : event.key); return;
        }
        // A ends a note by letting it release, which no piano key uses; the minus key stops it at once
        const ending = event.key.toLowerCase() === 'a' ? 'OFF' : event.key === '-' ? 'CUT' : null;
        const note = ending ?? keyboardNote(event.key, this.octave()); if (!note) return;
        event.preventDefault();
        this.writeNote(note, ending ? null : this.defaultVelocity(), true, event); return;
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

/** Saturated colours around the wheel. New tracks take the first eight in turn, so those are kept well apart. */
export const trackPalette = ['#d8352f', '#e8741c', '#e3b616', '#1fae4f', '#16a89a', '#1f9fd6', '#a352d9', '#d04a88', '#86ac1a', '#5fc8e8', '#6b72dc', '#8d9299'];
/** A track's own colour if it has one, otherwise a default from its position. */
export function trackColour(track: Track, index: number) { return track.colour ?? trackPalette[index % 8]; }
/** The colour of a column of the grid: a track's own, or the fixed colours of the returns and the master. */
/** The master's colour, wherever it is drawn. */
export const masterColour = '#f2f4f7';
export function columnColour(project: Song, column: number) {
  return column < project.tracks.length ? trackColour(project.tracks[column], column) : column === columnCount(project) - 1 ? masterColour : '#5fa8a0';
}
