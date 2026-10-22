import { laneParameter, targetKey, formatValue, parseValue, envelopeValue } from './automation';
import { keyboardNote, emptyFxLane, type FxTarget, type EffectParameters, type Project, type Pattern } from './project';
import { NoteHistory, parseNotes, type NoteEdit } from './history';

interface Lane { track: number; fx: number | null; values: (string | null)[] }

export class PatternGrid {
  private row = 0;
  private column = 0;
  private anchor = { row: 0, column: 0 };
  private histories = new Map<string, NoteHistory>();
  private targetSignatures = new Map<string, string>();
  private playing: number | null = null;
  private dragging = false;
  private dragged = false;

  constructor(
    private parent: HTMLElement,
    private project: () => Project,
    private pattern: () => Pattern,
    private octave: () => number,
    private edited: () => void,
    private assigned: (id: string | null) => void,
    private audition: (id: string, note: string, track: number) => void,
    private togglePlay: () => void,
    private trackSelected: (track: number) => void = () => {},
    private openDevice: (track: number, device: 'instrument' | 'effects', sameTrack: boolean) => void = () => {},
    private step: () => number = () => 1,
    private parameters: () => EffectParameters = () => ({}),
    private feedback: (text: string) => void = () => {},
    private openEnvelope: (track: number, fx: number) => void = () => {},
  ) {
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
    parent.addEventListener('cut', event => { if ((event.target as HTMLElement).closest('input')) return; this.copy(event); this.clearSelection(); });
    parent.addEventListener('paste', event => {
      if ((event.target as HTMLElement).closest('input')) return;
      const rows = parseNotes(event.clipboardData?.getData('text/plain') ?? ''); event.preventDefault();
      if (!rows) { this.feedback('Clipboard needs notes or parameter values, separated by tabs.'); return; }
      const lanes = this.lanes, edits: NoteEdit[] = [];
      for (const [r, values] of rows.entries()) for (const [c, raw] of values.entries()) {
        const row = Math.min(this.row, this.anchor.row) + r, column = Math.min(this.column, this.anchor.column) + c;
        if (row >= this.pattern().rows || column >= lanes.length) continue;
        if (lanes[column].fx !== null && this.pattern().fx[lanes[column].track][lanes[column].fx!].envelope?.enabled) { this.feedback('Disable Use envelope before pasting row values into this lane.'); return; }
        let value = raw;
        if (value !== null) {
          const lane = lanes[column], info = lane.fx === null ? null : this.info(lane);
          if (lane.fx !== null) value = info ? parseValue(value, info) : null;
          if (lane.fx === null ? !/^[A-G][#-][0-8]$/.test(value ?? '') : value === null) {
            this.feedback('Paste needs values in the destination lane’s units and range.'); return;
          }
        }
        edits.push({ row, track: column, before: lanes[column].values[row], after: value });
      }
      this.apply(edits);
    });
  }

  private get lanes(): Lane[] {
    return this.project().tracks.flatMap((_, track) => [
      { track, fx: null, values: this.pattern().notes[track] },
      ...this.pattern().fx[track].map((lane, fx) => ({ track, fx, values: lane.values })),
    ]);
  }
  private get values() { return this.lanes.map(lane => lane.values); }
  get activeTrack() { return this.lanes[this.column]?.track ?? 0; }
  private get history() {
    const pattern = this.pattern(), id = pattern.id;
    const signature = JSON.stringify(pattern.fx.map(track => track.map(lane => lane.target)));
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
  private id(row: number, lane: Lane) { return lane.fx === null ? `cell-${row}-${lane.track}` : `fx-${row}-${lane.track}-${lane.fx}`; }
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
    this.parent.setAttribute('aria-activedescendant', this.id(this.row, this.lanes[this.column]));
  }

  render() {
    const project = this.project(), pattern = this.pattern(), lanes = this.lanes;
    this.column = Math.min(this.column, lanes.length - 1); this.anchor.column = Math.min(this.anchor.column, lanes.length - 1);
    this.row = Math.min(this.row, pattern.rows - 1); this.anchor.row = Math.min(this.anchor.row, pattern.rows - 1);
    this.parent.replaceChildren();
    this.parent.style.setProperty('--track-columns', `42px ${pattern.fx.map(columns => `${Math.max(196, 72 + columns.length * 116)}px`).join(' ')}`);
    const header = document.createElement('div'); header.className = 'grid-row grid-header'; header.setAttribute('role', 'row');
    const label = document.createElement('span'); label.textContent = 'ROW'; label.setAttribute('role', 'columnheader'); header.append(label);
    for (const [index, track] of project.tracks.entries()) {
      const cell = document.createElement('div'); cell.setAttribute('role', 'columnheader'); cell.className = 'track-header'; cell.dataset.track = String(index);
      cell.style.setProperty('--track-colour', trackColour(index)); cell.classList.toggle('selected-track', index === this.activeTrack);
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
      for (const [device, text, label] of [['instrument', '{}', `Edit ${track.name} instrument`], ['effects', 'FX', `Show ${track.name} effects`]] as const) {
        const button = document.createElement('button'); button.textContent = text; button.title = label; button.setAttribute('aria-label', label);
        button.onclick = () => { const same = this.activeTrack === index; this.selectTrack(index); this.openDevice(index, device, same); }; tools.append(button);
      }
      const mute = document.createElement('button'); mute.textContent = 'M'; mute.title = `Mute ${track.name}`; mute.setAttribute('aria-label', mute.title); mute.setAttribute('aria-pressed', String(track.muted));
      mute.onclick = () => { track.muted = !track.muted; this.edited(); this.render(); }; tools.append(mute); heading.append(name, tools);
      const select = document.createElement('select'); select.setAttribute('aria-label', `${track.name} instrument`); select.add(new Option('No instrument', ''));
      for (const definition of project.definitions.filter(d => d.kind === 'instr')) select.add(new Option(definition.name, definition.id));
      select.value = track.instrument ?? ''; select.onchange = () => { track.instrument = select.value || null; this.assigned(track.instrument); this.edited(); };
      const columns = document.createElement('div'); columns.className = 'track-columns';
      const note = document.createElement('span'); note.textContent = 'NOTE'; columns.append(note);
      for (const [column, lane] of pattern.fx[index].entries()) {
        const picker = document.createElement('select'); picker.className = 'lane-target'; picker.setAttribute('aria-label', `${track.name} parameter lane ${column + 1}`);
        picker.add(new Option('Choose parameter…', ''));
        const choices: FxTarget[] = [{ kind: 'volume' }, { kind: 'pan' }];
        track.effects.forEach((slot, device) => { for (const p of this.parameters()[slot.definition] ?? []) choices.push({ kind: 'effect', device, parameter: p.name }); });
        for (const target of choices) picker.add(new Option(laneParameter(target, index, project, this.parameters())!.label, targetKey(target)));
        if (lane.target && !choices.some(t => targetKey(t) === targetKey(lane.target))) picker.add(new Option(`Apply FX to inspect ${lane.target.kind === 'effect' ? lane.target.parameter : 'parameter'}`, targetKey(lane.target)));
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
          if (delta > 0) pattern.fx[index].push({ target: null, values: Array(Math.max(...pattern.notes.map(notes => notes.length))).fill(null) });
          else if ((pattern.fx[index].at(-1)!.values.some(value => value !== null) || pattern.fx[index].at(-1)!.envelope?.points.length)) { this.feedback('Clear the last parameter lane before hiding it.'); return; }
          else pattern.fx[index].pop();
          this.resetHistory(); this.selectTrack(index); this.edited();
        }; adjust.append(button);
      }
      cell.append(heading, select, columns, adjust); cell.onkeydown = event => event.stopPropagation(); header.append(cell);
    }
    this.parent.append(header);
    for (let row = 0; row < pattern.rows; row++) {
      const line = document.createElement('div'); line.className = `grid-row ${row % project.rows_per_beat === 0 ? 'beat' : ''}`; line.setAttribute('role', 'row'); line.dataset.row = String(row);
      const number = document.createElement('span'); number.className = 'row-number'; number.textContent = String(row).padStart(2, '0'); line.append(number);
      let column = 0;
      for (let track = 0; track < project.tracks.length; track++) {
        const group = document.createElement('div'); group.className = 'track-cells';
        for (const lane of lanes.filter(lane => lane.track === track)) {
          const c = column++, value = lane.values[row], cell = document.createElement('div'); cell.setAttribute('role', 'gridcell');
          cell.className = `${lane.fx === null ? 'note' : 'fx-command'} ${value ? 'filled' : ''}`; cell.dataset.noteRow = String(row); cell.dataset.column = String(c);
          cell.id = this.id(row, lane); cell.textContent = value === null ? (lane.fx === null ? '···' : '·') : lane.fx === null ? value : formatValue(value, this.info(lane));
          if (lane.fx !== null && pattern.fx[track][lane.fx].envelope?.enabled) { const envelope = pattern.fx[track][lane.fx].envelope!; const v = envelopeValue(envelope, row, this.info(lane)?.unit === 'db'); cell.textContent = v === null ? '·' : formatValue(v, this.info(lane)); cell.classList.add('envelope-cell'); }
          if (lane.fx !== null) { cell.title = `${this.info(lane)?.label ?? 'Choose a parameter above'} · Click to edit; drag ⋮ vertically to adjust. Blank holds the previous value.`; if (value !== null && this.info(lane) && !pattern.fx[track][lane.fx].envelope?.enabled) { const handle = document.createElement('span'); handle.className = 'value-drag'; handle.textContent = '⋮'; handle.title = 'Drag vertically to adjust'; handle.onpointerdown = event => this.dragValue(event, cell, lane, row, c); cell.append(handle); } }
          cell.onclick = event => {
            if (this.dragged) { this.dragged = false; return; }
            if (!event.shiftKey) this.anchor = { row, column: c };
            this.row = row; this.column = c; this.trackSelected(track); this.render(); this.parent.focus();
            if (lane.fx !== null && !event.shiftKey) this.editValue(row, c);
          }; group.append(cell);
        }
        line.append(group);
      }
      this.parent.append(line);
    }
    this.paintSelection(); this.paint(this.playing);
    const cursor = this.parent.querySelector<HTMLElement>(`#${this.id(this.row, lanes[this.column])}`);
    cursor?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    if (cursor) { const cell = cursor.getBoundingClientRect(), viewport = this.parent.getBoundingClientRect(); if (cell.left < viewport.left + 42) this.parent.scrollLeft -= viewport.left + 42 - cell.left; }
  }

  paint(row: number | null) { this.playing = row; for (const line of this.parent.querySelectorAll<HTMLElement>('[data-row]')) line.classList.toggle('playing-row', row !== null && Number(line.dataset.row) === row); }
  private copy(event: ClipboardEvent) {
    if ((event.target as HTMLElement).closest('input')) return;
    const rows: string[] = [], lanes = this.lanes;
    for (let row = Math.min(this.row, this.anchor.row); row <= Math.max(this.row, this.anchor.row); row++) {
      const values: string[] = [];
      for (let c = Math.min(this.column, this.anchor.column); c <= Math.max(this.column, this.anchor.column); c++) {
        const lane = lanes[c], envelope = lane.fx === null ? undefined : this.pattern().fx[lane.track][lane.fx].envelope;
        const value = envelope?.enabled ? envelopeValue(envelope, row, this.info(lane)?.unit === 'db') : lane.values[row];
        values.push(value === null ? '---' : lane.fx === null ? String(value) : formatValue(value, this.info(lane)));
      }
      rows.push(values.join('\t'));
    }
    event.clipboardData?.setData('text/plain', rows.join('\n')); event.preventDefault();
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
      if (fx < 0 && columns.length < 4) { fx = columns.length; columns.push(emptyFxLane(this.pattern().notes[track].length)); }
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
      if (event.key.toLowerCase() === 'z') { event.preventDefault(); if (event.shiftKey) this.redo(); else this.undo(); }
      if (event.key.toLowerCase() === 'a') { event.preventDefault(); this.anchor = { row: 0, column: 0 }; this.row = this.pattern().rows - 1; this.column = this.lanes.length - 1; this.trackSelected(this.activeTrack); this.render(); } return;
    }
    if (event.altKey) return;
    const lanes = this.lanes, lane = lanes[this.column];
    switch (event.key) {
      case ' ': event.preventDefault(); this.togglePlay(); return;
      case 'Escape': this.render(); return;
      case 'ArrowUp': this.row = event.shiftKey ? Math.max(0, this.row - 1) : (this.row + this.pattern().rows - 1) % this.pattern().rows; break;
      case 'ArrowDown': this.row = event.shiftKey ? Math.min(this.pattern().rows - 1, this.row + 1) : (this.row + 1) % this.pattern().rows; break;
      case 'ArrowLeft': this.column = event.shiftKey ? Math.max(0, this.column - 1) : (this.column + lanes.length - 1) % lanes.length; break;
      case 'ArrowRight': this.column = event.shiftKey ? Math.min(lanes.length - 1, this.column + 1) : (this.column + 1) % lanes.length; break;
      case 'Tab': this.selectTrack((this.activeTrack + (event.shiftKey ? this.project().tracks.length - 1 : 1)) % this.project().tracks.length); event.preventDefault(); return;
      case 'Delete': case 'Backspace': event.preventDefault(); this.clearSelection(); return;
      default: {
        if (lane.fx !== null) {
          if (event.key === 'Enter' || event.key.length === 1 && /[0-9+.-]/.test(event.key)) { event.preventDefault(); this.editValue(this.row, this.column, event.key === 'Enter' ? undefined : event.key); } return;
        }
        const note = keyboardNote(event.key, this.octave()); if (!note) return;
        this.apply([{ row: this.row, track: this.column, before: lane.values[this.row], after: note }]);
        const instrument = this.project().tracks[lane.track].instrument; if (instrument) this.audition(instrument, note, lane.track);
        this.advance(); event.preventDefault(); this.render(); return;
      }
    }
    if (!event.shiftKey) this.anchor = { row: this.row, column: this.column };
    this.trackSelected(this.activeTrack); event.preventDefault(); this.render();
  }
}

export function trackColour(index: number) { return ['#c97979', '#c6a465', '#a9b966', '#73b48a', '#6db4be', '#829bd1', '#b28bce', '#cc8fb2'][index % 8]; }
