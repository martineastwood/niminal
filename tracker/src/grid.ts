import { patternTiming, patternSeconds, patternRow, patternSwing } from './project';
import { instrumentsOnTrack } from './instruments';
import { laneParameter, targetKey, formatValue, parseValue, envelopeValue } from './automation';
import { startValue, chanceValue, pitchValue, retriggerValue, lenValue, fxValue, glideRetriggers, keyboardNote, noteAt, noteNumber, transposeNote, emptyFxLane, blankColumn, subColumns, maxNoteColumns, columnCount, columnStrip, columnLanes, type FxTarget, type FxLane, type EffectParameters, type Project, type Song, type Pattern, type Track, type SubColumn } from './project';
import { type ExpressionKind, describe, fillExpressionEditor } from './expression';
import { NoteHistory, parseNotes, type NoteEdit } from './history';
import { openBrowser } from './browser';

/** What the grid asks of the sequence: move between steps, insert or remove one, or point the step at another pattern. */
export type SequenceAction = 'previous' | 'next' | 'first' | 'last' | 'new' | 'remove' | 'pattern-previous' | 'pattern-next';

const ROW_WIDTH = 30, NOTE_WIDTH = 44, SUB_WIDTH = 26, LANE_WIDTH = 72, MIN_TRACK_WIDTH = 120, MIN_STRIP_WIDTH = 84, MAX_LANES = 4;

/** A column of cells. Note-family lanes have `fx` null; parameter lanes have a `kind` of null. */
type Kind = 'note' | SubColumn;
interface Lane { track: number; kind: Kind | null; col: number; fx: number | null; values: (string | null)[] }
const subLabel: Record<SubColumn, string> = { inst: 'Ins', vol: 'Vol', pan: 'Pan', delay: 'Dly', start: 'Start', chance: 'Chance', pitch: 'Pitch', rtg: 'Rtg', len: 'Len', fx: 'Fx' };
const subTitle: Record<SubColumn, string> = {
  inst: 'Instrument for this note: its number in the sidebar list',
  vol: 'Volume for this note, 0 to 100',
  pan: 'Panning for this note, -50 (left) to +50 (right), C for centre',
  delay: 'Delay: how far into the row the note starts, as a percent, 0 to 99',
  start: 'Start: where the sample starts and which way it plays. 50 starts half way in, < plays it backwards, 50< does both',
  chance: 'Chance: the percent chance, 0 to 100, that this note plays; blank is certain. A note that does not play leaves the one before it sounding',
  pitch: 'Pitch: G8 glides the held note to this one over a row, U12 or D3 slides the held note up or down that many semitones during the row',
  rtg: 'Retrigger: how many times the note plays in its row, 2 to 32; 4:-25 makes each hit 25% quieter than the one before, 4:50 makes each 50% louder',
  len: 'Length: how long the note sounds before it is released, as a percent of a row, 1 to 99; blank holds it until the next note or OFF',
  fx: 'Effect: V-20 or V+10 moves the level of the note held in this column by that many percent of full level during the row',
};
const subRange: Record<SubColumn, string> = { inst: 'an instrument number', vol: '0 to 100', pan: '-50 to 50, or C', delay: '0 to 99', start: '0 to 99, <, or both such as 50<', chance: '0 to 100', pitch: 'G, U or D and a number, such as G8, U12 or D3', rtg: '2 to 32 hits, then : and a change in level from -100 to 100, such as 4:-25', len: '1 to 99', fx: 'V, then + or -, then 1 to 99, such as V-20' };

// Cells store the engine's whole numbers (volume and pan 0 to 128, delay 0 to 255); these show them as people count.
/** What a sub-column cell reads as, for display and the clipboard. */
export function subText(kind: SubColumn, value: string | null, project: Project): string {
  if (value === null) return '';
  if (kind === 'start' || kind === 'chance' || kind === 'pitch' || kind === 'rtg' || kind === 'len' || kind === 'fx') return value;
  const n = Number(value);
  if (kind === 'inst') { const index = project.definitions.filter(d => d.kind === 'instr').findIndex(d => d.id === value); return index < 0 ? '??' : String(index).padStart(2, '0'); }
  if (kind === 'vol') return String(Math.round(n * 100 / 128));
  if (kind === 'delay') return String(Math.min(99, Math.round(n * 100 / 256)));
  const pan = Math.round((n - 64) * 50 / 64);
  return pan === 0 ? 'C' : pan > 0 ? `+${pan}` : String(pan);
}
/** The stored value for typed or pasted text, or null when it names nothing. */
export function subValue(kind: SubColumn, text: string, project: Project): string | null {
  if (kind === 'start') return startValue(text);
  if (kind === 'chance') return chanceValue(text);
  if (kind === 'pitch') return pitchValue(text);
  if (kind === 'rtg') return retriggerValue(text);
  if (kind === 'len') return lenValue(text);
  if (kind === 'fx') return fxValue(text);
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
const expression = (kind: Kind | null) => kind === 'start' || kind === 'chance' || kind === 'pitch' || kind === 'rtg' || kind === 'len' || kind === 'fx';

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
  private dragged = false;

  constructor(
    private parent: HTMLElement,
    private project: () => Project,
    private pattern: () => Pattern,
    private octave: () => number,
    private edited: () => void,
    private audition: (id: string, note: string, track: number, expression?: { start?: string; retrigger?: string }) => void,
    private togglePlay: () => void,
    private trackSelected: (track: number) => void = () => {},
    private step: () => number = () => 1,
    private parameters: () => EffectParameters = () => ({}),
    private feedback: (text: string) => void = () => {},
    private openEnvelope: (track: number, fx: number) => void = () => {},
    private sequence: (action: SequenceAction) => void = () => {},
    /** The instrument that notes entered now are given: the one chosen in the sound browser. */
    private currentInstrument: () => string | null = () => null,
    /** The volume (0 to 100) given to notes entered in record mode, or null to leave it unset. */
    private defaultVelocity: () => number | null = () => null,
  ) {
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
      event.preventDefault(); this.dragging = true; this.dragged = false;
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
      this.dragged = true;
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
      if (!rows) { this.feedback('Clipboard needs notes or parameter values, separated by tabs.'); return; }
      // cells marked by a click are only there to copy from: pasting lands at the cursor
      const lanes = this.lanes, edits: NoteEdit[] = [], origin = this.marked ? { row: this.row, column: this.column } : { row: this.bounds.top, column: this.bounds.left };
      for (const [r, values] of rows.entries()) for (const [c, raw] of values.entries()) {
        const row = origin.row + r, column = origin.column + c;
        if (row >= this.pattern().rows || column >= lanes.length) continue;
        if (lanes[column].fx !== null && this.laneAt(lanes[column]).envelope?.enabled) { this.feedback('Disable Use envelope before pasting row values into this lane.'); return; }
        let value = raw;
        if (value !== null) {
          const lane = lanes[column];
          if (lane.fx !== null) { const info = this.info(lane); value = info ? parseValue(value, info) : null; }
          else if (lane.kind === 'note') value = /^([A-G][#-][0-8]|OFF|CUT)$/.test(value) ? value : null;
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
    const project = this.project();
    return Array.from({ length: columnCount(project) }, (_, track): Lane[] => [
      // returns and the master have no notes, only parameter lanes
      ...(track < project.tracks.length ? pattern.notes[track].flatMap((column, col): Lane[] => [
        { track, kind: 'note', col, fx: null, values: column.notes },
        ...subColumns.filter(kind => shows(project.tracks[track], kind)).map((kind): Lane => ({ track, kind, col, fx: null, values: column[kind] ??= [] })),
      ]) : []),
      ...columnLanes(project, pattern, track).map((lane, fx): Lane => ({ track, kind: null, col: 0, fx, values: lane.values })),
    ]).flat();
  }
  /** The parameter lane a lane of the grid shows. */
  private laneAt(lane: Lane): FxLane { return columnLanes(this.project(), this.pattern(), lane.track)[lane.fx!]; }
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
    if (lane.fx !== null) return `fx-${row}-${lane.track}-${lane.fx}`;
    return lane.kind === 'note' && lane.col === 0 ? `cell-${row}-${lane.track}` : `cell-${row}-${lane.track}-${lane.col}-${lane.kind}`;
  }
  private text(lane: Lane, value: string | null) { return lane.kind === 'note' || lane.kind === null ? value ?? '' : subText(lane.kind, value, this.project()); }
  /** The width of a column: a track's note columns with their sub-columns, then its parameter lanes. */
  private trackWidth(track: number) {
    const lanes = columnLanes(this.project(), this.pattern(), track).length * LANE_WIDTH;
    if (track >= this.project().tracks.length) return Math.max(MIN_STRIP_WIDTH, lanes + 2);
    const subs = subColumns.filter(kind => shows(this.project().tracks[track], kind)).length;
    return Math.max(MIN_TRACK_WIDTH, this.pattern().notes[track].length * (NOTE_WIDTH + subs * SUB_WIDTH) + 2 + lanes);
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
    // a lane for an instrument that no note on its track uses does nothing, so it is drawn dimmed
    const onTrack = project.tracks.map((_, track) => instrumentsOnTrack(project, track));
    const idle = (track: number, fx: number) => { const target = columnLanes(project, pattern, track)[fx].target; return target?.kind === 'instrument' && !onTrack[track]?.has(target.instrument); };
    this.column = Math.min(this.column, lanes.length - 1); this.anchor.column = Math.min(this.anchor.column, lanes.length - 1);
    this.row = Math.min(this.row, pattern.rows - 1); this.anchor.row = Math.min(this.anchor.row, pattern.rows - 1);
    this.parent.replaceChildren();
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
      const more = document.createElement('button'); more.textContent = '⋯'; more.title = isTrack ? `${track.name} note columns and parameter lanes` : `${track.name} parameter lanes`; more.setAttribute('aria-label', `${track.name} columns`);
      more.className = 'track-more'; more.onclick = () => { this.selectTrack(index, false); this.pickColumns(index, more); }; tools.append(more);
      // a dot after the name marks a track that has effects
      const effects = track.fx.length ? [Object.assign(document.createElement('span'), { className: 'has-fx', textContent: '●', title: `${track.fx.length} effect${track.fx.length > 1 ? 's' : ''}` })] : [];
      heading.append(name, ...effects, tools);
      const columns = document.createElement('div'); columns.className = 'track-columns';
      for (const lane of lanes.filter(lane => lane.track === index && lane.fx === null)) {
        const label = document.createElement('span'); label.textContent = lane.kind === 'note' ? 'Note' : subLabel[lane.kind!];
        label.className = lane.kind === 'note' ? 'col-label' : 'col-label sub-label'; if (lane.kind === 'note' && lane.col > 0) label.classList.add('col-start');
        if (lane.kind !== 'note') label.title = subTitle[lane.kind!]; columns.append(label);
      }
      for (const [column, lane] of columnLanes(project, pattern, index).entries()) {
        const picker = document.createElement('select'); picker.className = 'lane-target'; picker.setAttribute('aria-label', `${track.name} parameter lane ${column + 1}`);
        picker.add(new Option('—', ''));
        // short names so the lane stays narrow, grouped under what they belong to; the title has the full one
        const choices = this.laneChoices(index), groups = new Map<string, HTMLOptGroupElement>();
        for (const choice of choices) {
          if (!groups.has(choice.owner)) { const group = document.createElement('optgroup'); group.label = choice.owner; groups.set(choice.owner, group); picker.append(group); }
          groups.get(choice.owner)!.append(new Option(choice.name, targetKey(choice.target)));
        }
        if (lane.target && !choices.some(choice => targetKey(choice.target) === targetKey(lane.target))) picker.add(new Option(`Missing/unapplied parameter: ${lane.target.kind === 'effect' || lane.target.kind === 'instrument' ? lane.target.parameter : 'parameter'}`, targetKey(lane.target)));
        picker.value = targetKey(lane.target); picker.title = laneParameter(lane.target, index, project, this.parameters())?.label ?? 'Connect and Apply effects to expose more parameters';
        picker.onchange = () => { if (!this.setLaneTarget(index, lane, picker.value ? JSON.parse(picker.value) : null)) picker.value = targetKey(lane.target); };
        const control = document.createElement('div'); control.className = 'lane-heading';
        const graph = document.createElement('button'); graph.textContent = '⌁'; graph.title = `Edit ${picker.title} envelope`; graph.setAttribute('aria-label', `${track.name} lane ${column + 1} automation`); graph.onclick = () => this.openEnvelope(index, column);
        // lit while the lane is drawn by its envelope
        if (lane.envelope?.enabled) { graph.classList.add('on'); graph.title += ' (in use)'; }
        control.append(picker, graph); columns.append(control);
        if (idle(index, column)) { control.classList.add('idle'); picker.title += ' · no note on this track uses this instrument, so it has no effect'; }
      }
      cell.append(strip, heading, columns); cell.onkeydown = event => event.stopPropagation();
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
          cell.className = `${lane.fx !== null ? 'fx-command' : lane.kind === 'note' ? 'note' : 'sub'} ${value ? 'filled' : ''}`; cell.dataset.noteRow = String(row); cell.dataset.column = String(c);
          if (lane.kind === 'note' && lane.col > 0) cell.classList.add('col-start'); if (value === 'OFF') cell.classList.add('off'); if (value === 'CUT') cell.classList.add('cut');
          if (lane.fx !== null && idle(track, lane.fx)) cell.classList.add('idle');
          cell.id = this.id(row, lane); cell.textContent = value === null ? (lane.fx !== null ? '·' : lane.kind === 'note' ? '···' : '··') : lane.fx === null ? this.text(lane, value) : formatValue(value, this.info(lane), false, true);
          if (lane.fx !== null && columnLanes(project, pattern, track)[lane.fx].envelope?.enabled) { const envelope = columnLanes(project, pattern, track)[lane.fx].envelope!; const v = envelopeValue(envelope, row, this.info(lane)?.unit === 'db'); cell.textContent = v === null ? '·' : formatValue(v, this.info(lane), false, true); cell.classList.add('envelope-cell'); }
          if (lane.fx !== null) { cell.title = `${this.info(lane)?.label ?? 'Choose a parameter above'}${this.info(lane)?.unit ? ` (${this.info(lane)!.unit})` : ''} · Click to edit; drag ⋮ vertically to adjust. Blank holds the previous value. Press P to choose the parameter.`; if (value !== null && this.info(lane) && !columnLanes(project, pattern, track)[lane.fx].envelope?.enabled) { const handle = document.createElement('span'); handle.className = 'value-drag'; handle.textContent = '⋮'; handle.title = 'Drag vertically to adjust'; handle.onpointerdown = event => this.dragValue(event, cell, lane, row, c); cell.append(handle); } }
          cell.onclick = event => {
            if (this.dragged) { this.dragged = false; return; }
            if (!this.recording) return;
            if (!event.shiftKey) this.anchor = { row, column: c };
            this.row = row; this.column = c; this.trackSelected(track); this.render(); this.parent.focus();
            if (!event.shiftKey) { if (lane.fx !== null) this.editValue(row, c); else if (lane.kind !== 'note') this.editSub(row, c); }
          };
          if (expression(lane.kind)) {
            if (value != null) cell.title = describe(lane.kind as ExpressionKind, value) ?? '';
            // the clicks of a double click have opened the cell for typing; the editor takes over from that
            cell.ondblclick = () => { if (this.recording) { (document.activeElement as HTMLElement | null)?.blur(); this.openEditor(row, c); } };
          }
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
  /** What a lane on `track` can drive: its volume and pan, its effects' parameters, and those of the instruments it plays (and any a lane already points at). */
  private laneChoices(track: number): { target: FxTarget; owner: string; name: string }[] {
    const project = this.project(), strip = columnStrip(project, track);
    // only a track has notes, so only a track has instruments to automate
    const listed = new Set(track < project.tracks.length ? [...instrumentsOnTrack(project, track), ...this.pattern().fx[track].flatMap(lane => lane.target?.kind === 'instrument' ? [lane.target.instrument] : [])] : []);
    const choices: { target: FxTarget; owner: string; name: string }[] = [{ target: { kind: 'volume' }, owner: 'Track', name: 'Vol' }, { target: { kind: 'pan' }, owner: 'Track', name: 'Pan' }];
    for (const definition of project.definitions) if (definition.kind === 'instr' && listed.has(definition.id)) for (const p of this.parameters()[definition.id] ?? []) choices.push({ target: { kind: 'instrument', instrument: definition.id, parameter: p.name }, owner: definition.name, name: p.name });
    strip.macros?.forEach((knob, index) => choices.push({ target: { kind: 'macro', index }, owner: 'Macros', name: knob.name || `Macro ${index + 1}` }));
    const driven = (device: string, parameter: string) => strip.macros?.some(knob => knob.links.some(link => link.device === device && link.parameter === parameter));
    for (const device of strip.fx) for (const p of (this.parameters()[device.definition] ?? []).filter(p => !driven(device.id, p.name))) choices.push({ target: { kind: 'effect', device: device.id, parameter: p.name }, owner: project.definitions.find(d => d.id === device.definition)?.name ?? 'Effect', name: p.name });
    return choices;
  }
  /** Points a lane at a parameter. Says why and returns false if the lane has values to lose or the parameter already has a lane on this track. */
  private setLaneTarget(track: number, lane: FxLane, target: FxTarget | null): boolean {
    if (lane.values.some(value => value !== null) || lane.envelope?.points.length) { this.feedback('Clear this lane’s values before choosing a different parameter.'); return false; }
    if (target && columnLanes(this.project(), this.pattern(), track).some(other => other !== lane && targetKey(other.target) === targetKey(target))) { this.feedback('That parameter already has a lane in this track.'); return false; }
    lane.target = target; this.resetHistory(); this.edited(); this.render();
    return true;
  }
  /** P on a parameter lane: a list of what it can drive, with a search box, so the choice needs no mouse. */
  private chooseLaneTarget(lane: Lane) {
    const anchor = this.parent.querySelectorAll<HTMLElement>(`.track-header[data-track="${lane.track}"] .lane-target`)[lane.fx!];
    const choices = this.laneChoices(lane.track);
    openBrowser(anchor, [{ value: '', name: 'No parameter', folder: '' }, ...choices.map(choice => ({ value: targetKey(choice.target), name: choice.name, folder: choice.owner }))], item => {
      this.setLaneTarget(lane.track, this.laneAt(lane), item.value ? JSON.parse(item.value) : null);
      this.parent.focus();
    }, this.parent);
  }
  /** Adds a parameter lane to a track in this pattern, or hides its last one if that is empty. Says why and returns false if it cannot. */
  private changeLanes(index: number, delta: 1 | -1): boolean {
    const pattern = this.pattern(), lanes = columnLanes(this.project(), pattern, index);
    if (delta > 0) lanes.push({ target: null, values: Array(Math.max(...pattern.notes.flat().map(column => column.notes.length))).fill(null) });
    else if (lanes.at(-1)!.values.some(value => value !== null) || lanes.at(-1)!.envelope?.points.length) { this.feedback('Clear the last parameter lane before hiding it.'); return false; }
    else lanes.pop();
    return true;
  }
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
      if (project.patterns.some(p => { const last = p.notes[index].at(-1)!; return [last.notes, last.inst, last.vol, last.pan, last.delay, last.start, last.pitch, last.rtg, last.len, last.fx].some(cells => cells.some(v => v !== null)); })) { this.feedback('Clear the last note column in every pattern before removing it.'); return; }
      for (const p of project.patterns) p.notes[index].pop();
      changed();
    };
    const amount = document.createElement('strong'); amount.textContent = String(count);
    const more = document.createElement('button'); more.textContent = '+'; more.setAttribute('aria-label', 'More note columns'); more.disabled = count >= maxNoteColumns;
    more.onclick = () => { for (const p of project.patterns) p.notes[index].push(blankColumn(Math.max(p.rows, p.notes[index][0].notes.length))); changed(); };
    // a return or the master has no notes, so only a track has note columns and sub-columns to set
    const line = (...nodes: Node[]) => { const div = document.createElement('div'); div.className = 'choice-row'; div.append(...nodes); return div; };
    if (isTrack) element.append(line(heading, fewer, amount, more));
    const lanes = columnLanes(project, this.pattern(), index).length;
    const laneHeading = document.createElement('span'); laneHeading.textContent = 'Parameter lanes';
    const fewerLanes = document.createElement('button'); fewerLanes.textContent = '−'; fewerLanes.setAttribute('aria-label', 'Fewer parameter lanes'); fewerLanes.disabled = lanes <= 1;
    fewerLanes.onclick = () => { if (this.changeLanes(index, -1)) changed(); };
    const laneCount = document.createElement('strong'); laneCount.textContent = String(lanes);
    const moreLanes = document.createElement('button'); moreLanes.textContent = '+'; moreLanes.setAttribute('aria-label', 'More parameter lanes'); moreLanes.disabled = lanes >= MAX_LANES;
    moreLanes.onclick = () => { if (this.changeLanes(index, 1)) changed(); };
    element.append(line(laneHeading, fewerLanes, laneCount, moreLanes));
    const checks = document.createElement('div'); checks.className = 'choice-checks'; if (isTrack) element.append(checks);
    for (const kind of isTrack ? subColumns.filter(kind => kind !== 'inst') : []) {
      const label = document.createElement('label'); label.title = subTitle[kind];
      const box = document.createElement('input'); box.type = 'checkbox'; box.checked = !!track.show?.[kind]; box.setAttribute('aria-label', `Show ${subLabel[kind]} column`);
      box.onchange = () => { track.show = { ...track.show, [kind]: box.checked }; changed(); };
      label.append(box, { vol: 'Volume', pan: 'Panning', delay: 'Delay', start: 'Start', chance: 'Chance', pitch: 'Pitch', rtg: 'Retrigger', len: 'Length', fx: 'Effect' }[kind]); checks.append(label);
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
      const notes = this.pattern().notes[lane.track]?.[lane.col];
      const competing = commit && value !== null && notes && (kind === 'pitch' ? glideRetriggers(value, notes.rtg[row]) : kind === 'rtg' && glideRetriggers(notes.pitch[row], value));
      if (competing) { this.feedback('A glide and a retrigger cannot share a row: clear one first.'); if (advance) { input.focus(); return; } commit = false; }
      if (commit && !blank && value === null) { this.feedback(kind === 'inst' ? 'No instrument has that number.' : `Enter ${subRange[kind]}.`); if (advance) { input.focus(); return; } commit = false; }
      finished = true; this.keepView = !advance;
      if (commit) this.apply([{ row, track: column, before: lane.values[row], after: value }]);
      if (commit && (kind === 'start' || kind === 'rtg') && value) this.previewCommand(row, lane, value);
      if (advance && commit) this.advance(); this.render(); this.keepView = false; if (advance) this.parent.focus();
    };
    input.onkeydown = event => { event.stopPropagation(); if (event.key === 'Enter' || event.key === 'Escape' || event.key === 'Tab') { event.preventDefault(); finish(event.key !== 'Escape', true); } };
    input.onblur = () => finish(true, false);
  }
  /** The popup editor for a start, chance, pitch or retrigger cell: named fields, a reading in words, Apply and Cancel. */
  private openEditor(row: number, column: number) {
    const lane = this.lanes[column];
    if (!this.editable(lane) || !expression(lane.kind)) return;
    const kind = lane.kind as ExpressionKind, notes = this.pattern().notes[lane.track]?.[lane.col];
    const anchor = this.parent.querySelector<HTMLElement>(`#${this.id(row, lane)}`);
    if (!notes || !anchor) return;
    const otherKind = kind === 'pitch' ? 'rtg' : kind === 'rtg' ? 'pitch' : null;
    const other = otherKind ? notes[otherKind][row] : null;
    const otherLane = otherKind ? this.lanes.findIndex(l => l.track === lane.track && l.col === lane.col && l.kind === otherKind) : -1;
    const { element, close } = this.popup(anchor, 'expression-editor', true);
    const closeAndFocus = () => { close(); this.parent.focus(); };
    fillExpressionEditor(element, closeAndFocus, kind, lane.values[row], other, otherLane >= 0, (value, clearOther) => {
      const edits: NoteEdit[] = [{ row, track: column, before: lane.values[row], after: value }];
      if (clearOther && otherLane >= 0) edits.push({ row, track: otherLane, before: other, after: null });
      this.apply(edits); this.render();
      if (value !== null && (kind === 'start' || kind === 'rtg')) this.previewCommand(row, lane, value);
    });
  }
  /** Plays the note a command at `row` acts on, with the command: the note in the row, or the one held above it. */
  private previewCommand(row: number, lane: Lane, value: string) {
    const column = this.pattern().notes[lane.track]?.[lane.col]; if (!column) return;
    for (let at = row; at >= 0; at--) {
      const note = column.notes[at];
      if (note === null) continue;
      if (noteNumber(note) === null) return;
      const instrument = instrumentAt(column.inst, at);
      if (instrument) this.audition(instrument, note, lane.track, lane.kind === 'start' ? { start: value } : { retrigger: value });
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
        const lane = lanes[c], envelope = lane.fx === null ? undefined : this.laneAt(lane).envelope;
        const value = envelope?.enabled ? envelopeValue(envelope, row, this.info(lane)?.unit === 'db') : lane.values[row];
        values.push(value === null ? '---' : lane.fx === null ? this.text(lane, String(value)) : formatValue(value, this.info(lane), true));
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
      const lane = this.lanes[column]; if (!this.editable(lane)) continue;
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
      const lane = this.lanes[column]; if (!this.editable(lane) || lane.kind === 'inst' || expression(lane.kind)) continue;
      const number = (value: string | null) => value === null ? null : lane.kind === 'note' ? noteNumber(value) : Number.isFinite(Number(value)) ? Number(value) : null;
      const rows = Array.from({ length: bottom - top + 1 }, (_, i) => top + i).filter(row => number(lane.values[row]) !== null);
      const numbers = rows.map(row => number(lane.values[row])!), low = Math.min(...numbers), high = Math.max(...numbers);
      if (rows.length < 2 || low === high) continue;
      for (const row of rows) {
        const pick = low + this.random() * (high - low);
        const after = lane.kind === 'note' ? noteAt(Math.round(pick)) : String(lane.fx === null ? Math.round(pick) : Number(pick.toPrecision(8)));
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
  /** Lanes drawn by a graphical envelope have no cells of their own to edit. */
  private editable(lane: Lane) { return lane.fx === null || !this.laneAt(lane).envelope?.enabled; }
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
      if (lane.kind === 'note' || lane.kind === 'inst' || expression(lane.kind) || !this.editable(lane)) continue;
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
    this.lanes.forEach((lane, column) => { if (lane.fx !== null && this.laneAt(lane).envelope?.enabled) return; for (let row = 0; row < this.pattern().rows; row++) if (this.selected(row, column)) edits.push({ row, track: column, before: lane.values[row], after: null }); }); this.apply(edits);
  }
  private advance() { this.row = (this.row + Math.max(0, Math.min(256, Math.trunc(this.step()) || 0))) % this.pattern().rows; this.anchor = { row: this.row, column: this.column }; }
  private info(lane: Lane) { return laneParameter(lane.fx === null ? null : this.laneAt(lane).target, lane.track, this.project(), this.parameters()); }
  private editValue(row: number, column: number, initial?: string) {
    const lane = this.lanes[column];
    if (lane.fx !== null && this.laneAt(lane).envelope?.enabled) { this.openEnvelope(lane.track, lane.fx); return; }
    const info = this.info(lane), cell = this.parent.querySelector<HTMLElement>(`#${this.id(row, lane)}`);
    if (!info || !cell) { this.feedback('Choose a lane parameter first. Connect and Apply an effect to expose its controls.'); return; }
    const input = document.createElement('input'); input.className = 'lane-value'; input.setAttribute('aria-label', `${info.label} row ${row}`);
    input.value = initial ?? (lane.values[row] === null ? '' : formatValue(lane.values[row]!, info, false, true));
    const originalText = input.value;
    input.placeholder = formatValue(info.default, info, false, true); cell.replaceChildren(input); input.focus(); if (initial === undefined) input.select();
    let finished = false;
    const finish = (commit: boolean, advance: boolean) => {
      if (finished) return;
      const value = initial === undefined && input.value === originalText ? lane.values[row] : input.value.trim() === '' ? null : parseValue(input.value, info);
      if (commit && input.value.trim() && value === null) { this.feedback(`Enter ${formatValue(info.min, info)} to ${formatValue(info.max, info)}.`); if (advance) { input.focus(); return; } commit = false; }
      finished = true; this.keepView = !advance;
      if (commit) this.apply([{ row, track: column, before: lane.values[row], after: value }]);
      if (advance && commit) this.advance(); this.render(); this.keepView = false; if (advance) this.parent.focus();
    };
    input.onkeydown = event => { event.stopPropagation(); if (event.key === 'Enter' || event.key === 'Escape' || event.key === 'Tab') { event.preventDefault(); finish(event.key !== 'Escape', true); } };
    input.onblur = () => finish(true, false);
  }
  private dragValue(event: PointerEvent, cell: HTMLElement, lane: Lane, row: number, column: number) {
    const info = this.info(lane); if (!info) return;
    event.preventDefault(); event.stopPropagation();
    const handle = event.currentTarget as HTMLElement, before = lane.values[row], start = Number(before ?? info.default), y = event.clientY;
    let value = start; handle.setPointerCapture?.(event.pointerId);
    handle.onpointermove = move => { value = Math.min(info.max, Math.max(info.min, start + (y - move.clientY) * (info.max - info.min) / (move.shiftKey ? 2000 : 200))); cell.firstChild!.textContent = formatValue(value, info, false, true); };
    const finish = (commit: boolean) => { handle.onpointermove = null; handle.onpointerup = null; handle.onpointercancel = null; if (commit) this.apply([{ row, track: column, before, after: String(Number(value.toPrecision(8))) }]); else this.render(); };
    handle.onpointerup = up => { up.stopPropagation(); finish(true); }; handle.onpointercancel = () => finish(false); handle.onclick = click => click.stopPropagation();
  }
  writeParameter(track: number, target: FxTarget, value: number) {
    const columns = columnLanes(this.project(), this.pattern(), track); let fx = columns.findIndex(lane => targetKey(lane.target) === targetKey(target));
    if (fx < 0) {
      fx = columns.findIndex(lane => lane.values.every(value => value === null) && lane.target === null);
      if (fx < 0 && columns.length < 4) { fx = columns.length; columns.push(emptyFxLane(this.pattern().notes[0][0].notes.length)); }
      if (fx < 0) { this.feedback('Add or clear a parameter lane before recording this control (maximum four per track).'); return; }
      columns[fx].target = { ...target }; this.resetHistory();
    }
    const envelope = columns[fx].envelope; if (envelope) envelope.enabled = false;
    const column = this.lanes.findIndex(lane => lane.track === track && lane.fx === fx); this.column = column;
    this.apply([{ row: this.row, track: column, before: columns[fx].values[this.row], after: String(value) }]);
    this.advance(); this.render(); this.parent.focus();
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

  private writeNote(note: string, velocity: number | null, preview = true) {
    const lane = this.lanes[this.column], ending = note === 'OFF' || note === 'CUT';
    const live = this.live; if (live) { this.row = this.nearestRow(); this.anchor = { row: this.row, column: this.column }; }
    const column = this.pattern().notes[lane.track][lane.col], inst = ending ? undefined : this.currentInstrument();
    const instrument = !ending && (inst ?? instrumentAt(column.inst, this.row));
    if (instrument && preview) this.audition(instrument, note, lane.track);
    const track = this.project().tracks[lane.track];
    if (velocity !== null && !track.show?.vol) track.show = { ...track.show, vol: true };
    this.column = this.lanes.findIndex(other => other.track === lane.track && other.col === lane.col && other.kind === 'note');
    const instLane = this.lanes.findIndex(other => other.track === lane.track && other.col === lane.col && other.kind === 'inst');
    const volLane = this.lanes.findIndex(other => other.track === lane.track && other.col === lane.col && other.kind === 'vol');
    this.apply([{ row: this.row, track: this.column, before: lane.values[this.row], after: note }, ...(inst ? [{ row: this.row, track: instLane, before: column.inst[this.row], after: inst }] : []), ...(velocity !== null && volLane >= 0 ? [{ row: this.row, track: volLane, before: column.vol[this.row], after: String(Math.round(velocity * 1.28)) }] : [])]);
    if (!live) this.advance();
    this.render();
  }

  /** Plays the key as a note on the cursor's track while focus is outside the grid, so the keyboard sounds from anywhere. */
  auditionKey(event: KeyboardEvent) {
    if (event.ctrlKey || event.metaKey || event.altKey || event.repeat) return;
    const note = keyboardNote(event.key, this.octave()), instrument = this.currentInstrument();
    if (!note || !instrument) return;
    event.preventDefault();
    this.audition(instrument, note, Math.min(this.lanes[this.column].track, this.project().tracks.length - 1));
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
          if (instrument) this.audition(instrument, note, track);
          return;
        }
        if (lane.fx !== null) {
          if (event.key.toLowerCase() === 'p' && !event.shiftKey) { event.preventDefault(); this.chooseLaneTarget(lane); return; }
          if (event.key === 'Enter' || event.key.length === 1 && /[0-9+.-]/.test(event.key)) { event.preventDefault(); this.editValue(this.row, this.column, event.key === 'Enter' ? undefined : event.key); } return;
        }
        if (lane.kind !== 'note') {
          if (event.key === 'Enter' && expression(lane.kind)) { event.preventDefault(); this.openEditor(this.row, this.column); return; }
          if (!(event.key === 'Enter' || /^[0-9+-]$/.test(event.key) || lane.kind === 'pan' && /^c$/i.test(event.key) || lane.kind === 'start' && event.key === '<' || lane.kind === 'pitch' && /^[gud]$/i.test(event.key))) return;
          event.preventDefault(); this.editSub(this.row, this.column, event.key === 'Enter' ? undefined : event.key); return;
        }
        // A ends a note by letting it release, which no piano key uses; the minus key stops it at once
        const ending = event.key.toLowerCase() === 'a' ? 'OFF' : event.key === '-' ? 'CUT' : null;
        const note = ending ?? keyboardNote(event.key, this.octave()); if (!note) return;
        event.preventDefault();
        this.writeNote(note, ending ? null : this.defaultVelocity()); return;
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
