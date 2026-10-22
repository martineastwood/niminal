import { keyboardNote, type Project, type Pattern } from './project';
import { NoteHistory, parseNotes, type NoteEdit } from './history';

export class PatternGrid {
  private row = 0;
  private track = 0;
  private anchor = { row: 0, track: 0 };
  private histories = new Map<string, NoteHistory>();
  private playing: number | null = null;

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
    private openDevice: (track: number, device: 'instrument' | 'effects') => void = () => {},
  ) {
    parent.addEventListener('keydown', event => this.keydown(event));
    parent.addEventListener('copy', event => this.copy(event));
    parent.addEventListener('cut', event => { this.copy(event); this.clearSelection(); });
    parent.addEventListener('paste', event => {
      const rows = parseNotes(event.clipboardData?.getData('text/plain') ?? '');
      event.preventDefault();
      if (!rows) return;
      const edits: NoteEdit[] = [];
      for (const [r, notes] of rows.entries()) for (const [t, note] of notes.entries()) {
        const row = Math.min(this.row, this.anchor.row) + r, track = Math.min(this.track, this.anchor.track) + t;
        if (row < this.pattern().rows && track < this.project().tracks.length) edits.push({ row, track, before: this.pattern().notes[track][row], after: note });
      }
      if (this.history.apply(this.pattern().notes, edits)) { this.render(); this.edited(); }
    });
  }

  render() {
    const project = this.project();
    const pattern = this.pattern();
    this.row = Math.min(this.row, pattern.rows - 1);
    this.anchor.row = Math.min(this.anchor.row, pattern.rows - 1);
    this.parent.replaceChildren();
    this.parent.style.setProperty('--track-count', String(project.tracks.length));
    const header = document.createElement('div');
    header.className = 'grid-row grid-header'; header.setAttribute('role', 'row');
    const label = document.createElement('span'); label.textContent = 'ROW'; label.setAttribute('role', 'columnheader'); header.append(label);
    for (const [index, track] of project.tracks.entries()) {
      const cell = document.createElement('div'); cell.setAttribute('role', 'columnheader'); cell.className = 'track-header'; cell.dataset.track = String(index);
      cell.style.setProperty('--track-colour', trackColour(index)); cell.classList.toggle('selected-track', index === this.track);
      const name = document.createElement('button'); name.className = 'track-name'; name.textContent = `${String(index + 1).padStart(2, '0')} ${track.name}`;
      name.onclick = () => this.selectTrack(index);
      const tools = document.createElement('div'); tools.className = 'track-tools';
      for (const [device, text, label] of [['instrument', '{}', `Edit ${track.name} instrument`], ['effects', 'FX', `Show ${track.name} effects`]] as const) {
        const button = document.createElement('button'); button.textContent = text; button.title = label; button.setAttribute('aria-label', label);
        button.onclick = () => { this.selectTrack(index); this.openDevice(index, device); }; tools.append(button);
      }
      const mute = document.createElement('button'); mute.textContent = 'M'; mute.title = `Mute ${track.name}`; mute.setAttribute('aria-label', mute.title); mute.setAttribute('aria-pressed', String(track.muted));
      mute.onclick = () => { track.muted = !track.muted; this.edited(); this.render(); }; tools.append(mute);
      const select = document.createElement('select'); select.setAttribute('aria-label', `${track.name} instrument`);
      select.add(new Option('No instrument', ''));
      for (const definition of project.definitions.filter(d => d.kind === 'instr')) select.add(new Option(definition.name, definition.id));
      select.value = track.instrument ?? '';
      select.onchange = () => { track.instrument = select.value || null; this.assigned(track.instrument); this.edited(); };
      // Typing in a selector must never enter notes into the pattern.
      select.onkeydown = event => event.stopPropagation();
      const heading = document.createElement('div'); heading.className = 'track-heading'; heading.append(name, tools);
      const columns = document.createElement('div'); columns.className = 'track-columns'; columns.textContent = 'NOTE';
      cell.append(heading, select, columns);
      cell.onkeydown = event => event.stopPropagation(); header.append(cell);
    }
    this.parent.append(header);
    for (let r = 0; r < pattern.rows; r++) {
      const line = document.createElement('div'); line.className = `grid-row ${r % 4 === 0 ? 'beat' : ''}`; line.setAttribute('role', 'row'); line.dataset.row = String(r);
      const number = document.createElement('span'); number.className = 'row-number'; number.textContent = String(r).padStart(2, '0'); line.append(number);
      for (let t = 0; t < project.tracks.length; t++) {
        const cell = document.createElement('div'); cell.setAttribute('role', 'gridcell');
        cell.className = `note ${pattern.notes[t][r] ? 'filled' : ''} ${this.isSelected(r, t) ? 'range-selected' : ''} ${r === this.row && t === this.track ? 'cursor' : ''}`;
        cell.id = `cell-${r}-${t}`; cell.setAttribute('aria-selected', String(this.isSelected(r, t)));
        cell.textContent = pattern.notes[t][r] ?? '···';
        cell.onclick = event => { if (!event.shiftKey) this.anchor = { row: r, track: t }; this.row = r; this.track = t; this.trackSelected(t); this.render(); this.parent.focus(); };
        line.append(cell);
      }
      this.parent.append(line);
    }
    this.parent.setAttribute('aria-activedescendant', `cell-${this.row}-${this.track}`);
    const cursor = this.parent.querySelector<HTMLElement>(`#cell-${this.row}-${this.track}`);
    cursor?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    // Leave room for the pinned row gutter when revealing a track to the left.
    if (cursor) {
      const cell = cursor.getBoundingClientRect(), viewport = this.parent.getBoundingClientRect();
      if (cell.left < viewport.left + 42) this.parent.scrollLeft -= viewport.left + 42 - cell.left;
      else if (cell.right > viewport.right) this.parent.scrollLeft += cell.right - viewport.right;
    }
    this.paint(this.playing);
  }

  paint(row: number | null) {
    this.playing = row;
    for (const line of this.parent.querySelectorAll<HTMLElement>('[data-row]')) line.classList.toggle('playing-row', row !== null && Number(line.dataset.row) === row);
  }

  private get history() {
    const id = this.pattern().id;
    if (!this.histories.has(id)) this.histories.set(id, new NoteHistory());
    return this.histories.get(id)!;
  }

  get activeTrack() { return this.track; }

  selectTrack(track: number) {
    this.track = Math.max(0, Math.min(this.project().tracks.length - 1, track));
    this.anchor = { row: this.row, track: this.track }; this.trackSelected(this.track); this.render();
  }

  resetHistory() { this.histories.clear(); this.anchor = { row: this.row, track: this.track }; }
  undo() { if (this.history.undo(this.pattern().notes)) { this.render(); this.edited(); } }
  redo() { if (this.history.redo(this.pattern().notes)) { this.render(); this.edited(); } }

  private isSelected(row: number, track: number) {
    return row >= Math.min(this.row, this.anchor.row) && row <= Math.max(this.row, this.anchor.row) && track >= Math.min(this.track, this.anchor.track) && track <= Math.max(this.track, this.anchor.track);
  }

  private copy(event: ClipboardEvent) {
    const lines: string[] = [];
    for (let r = Math.min(this.row, this.anchor.row); r <= Math.max(this.row, this.anchor.row); r++) {
      const notes: string[] = [];
      for (let t = Math.min(this.track, this.anchor.track); t <= Math.max(this.track, this.anchor.track); t++) notes.push(this.pattern().notes[t][r] ?? '---');
      lines.push(notes.join('\t'));
    }
    event.clipboardData?.setData('text/plain', lines.join('\n')); event.preventDefault();
  }

  private clearSelection() {
    const edits: NoteEdit[] = [];
    for (let row = 0; row < this.pattern().rows; row++) for (let track = 0; track < this.project().tracks.length; track++) {
      if (this.isSelected(row, track)) edits.push({ row, track, before: this.pattern().notes[track][row], after: null });
    }
    if (this.history.apply(this.pattern().notes, edits)) { this.render(); this.edited(); }
  }

  private keydown(event: KeyboardEvent) {
    if (event.ctrlKey || event.metaKey) {
      if (event.key.toLowerCase() === 'z') { event.preventDefault(); if (event.shiftKey) this.redo(); else this.undo(); }
      if (event.key.toLowerCase() === 'a') { event.preventDefault(); this.anchor = { row: 0, track: 0 }; this.row = this.pattern().rows - 1; this.track = this.project().tracks.length - 1; this.render(); }
      return;
    }
    if (event.altKey) return;
    switch (event.key) {
      case ' ': event.preventDefault(); this.togglePlay(); return;
      case 'ArrowUp': this.row = event.shiftKey ? Math.max(0, this.row - 1) : (this.row + this.pattern().rows - 1) % this.pattern().rows; break;
      case 'ArrowDown': this.row = event.shiftKey ? Math.min(this.pattern().rows - 1, this.row + 1) : (this.row + 1) % this.pattern().rows; break;
      case 'ArrowLeft': this.track = event.shiftKey ? Math.max(0, this.track - 1) : (this.track + this.project().tracks.length - 1) % this.project().tracks.length; break;
      case 'ArrowRight': this.track = event.shiftKey ? Math.min(this.project().tracks.length - 1, this.track + 1) : (this.track + 1) % this.project().tracks.length; break;
      case 'Delete': case 'Backspace': event.preventDefault(); this.clearSelection(); return;
      default: {
        const note = keyboardNote(event.key, this.octave());
        if (!note) return;
        const track = this.project().tracks[this.track];
        this.history.apply(this.pattern().notes, [{ row: this.row, track: this.track, before: this.pattern().notes[this.track][this.row], after: note }]);
        if (track.instrument) this.audition(track.instrument, note, this.track);
        this.row = (this.row + 1) % this.pattern().rows;
        this.anchor = { row: this.row, track: this.track };
        event.preventDefault(); this.render(); this.edited(); return;
      }
    }
    if (!event.shiftKey) this.anchor = { row: this.row, track: this.track };
    this.trackSelected(this.track);
    event.preventDefault(); this.render();
  }
}

export function trackColour(index: number) {
  return ['#c97979', '#c6a465', '#a9b966', '#73b48a', '#6db4be', '#829bd1', '#b28bce', '#cc8fb2'][index % 8];
}
