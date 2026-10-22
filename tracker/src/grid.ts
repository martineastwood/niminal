import { keyboardNote, type Project } from './project';
import { NoteHistory, parseNotes, type NoteEdit } from './history';

export class PatternGrid {
  private row = 0;
  private track = 0;
  private anchor = { row: 0, track: 0 };
  private history = new NoteHistory();
  private playing: number | null = null;

  constructor(
    private parent: HTMLElement,
    private project: () => Project,
    private octave: () => number,
    private edited: () => void,
    private assigned: (id: string | null) => void,
    private audition: (id: string, note: string, track: number) => void,
    private togglePlay: () => void,
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
        if (row < 32 && track < 4) edits.push({ row, track, before: this.project().tracks[track].notes[row], after: note });
      }
      if (this.history.apply(this.project().tracks, edits)) { this.render(); this.edited(); }
    });
  }

  render() {
    const project = this.project();
    this.parent.replaceChildren();
    const header = document.createElement('div');
    header.className = 'grid-row grid-header'; header.setAttribute('role', 'row');
    const label = document.createElement('span'); label.textContent = 'ROW'; label.setAttribute('role', 'columnheader'); header.append(label);
    for (const [index, track] of project.tracks.entries()) {
      const cell = document.createElement('div'); cell.setAttribute('role', 'columnheader');
      const name = document.createElement('span'); name.textContent = `${index + 1} / ${track.name}`;
      const select = document.createElement('select'); select.setAttribute('aria-label', `${track.name} instrument`);
      select.add(new Option('No instrument', ''));
      for (const definition of project.definitions.filter(d => d.kind === 'instr')) select.add(new Option(definition.name, definition.id));
      select.value = track.instrument ?? '';
      select.onchange = () => { track.instrument = select.value || null; this.assigned(track.instrument); this.edited(); };
      // Typing in a selector must never enter notes into the pattern.
      select.onkeydown = event => event.stopPropagation();
      cell.append(name, select); header.append(cell);
    }
    this.parent.append(header);
    for (let r = 0; r < 32; r++) {
      const line = document.createElement('div'); line.className = `grid-row ${r % 4 === 0 ? 'beat' : ''}`; line.setAttribute('role', 'row'); line.dataset.row = String(r);
      const number = document.createElement('span'); number.className = 'row-number'; number.textContent = String(r).padStart(2, '0'); line.append(number);
      for (let t = 0; t < 4; t++) {
        const cell = document.createElement('div'); cell.setAttribute('role', 'gridcell');
        cell.className = `note ${project.tracks[t].notes[r] ? 'filled' : ''} ${this.isSelected(r, t) ? 'range-selected' : ''} ${r === this.row && t === this.track ? 'cursor' : ''}`;
        cell.id = `cell-${r}-${t}`; cell.setAttribute('aria-selected', String(this.isSelected(r, t)));
        cell.textContent = project.tracks[t].notes[r] ?? '···';
        cell.onclick = event => { if (!event.shiftKey) this.anchor = { row: r, track: t }; this.row = r; this.track = t; this.render(); this.parent.focus(); };
        line.append(cell);
      }
      this.parent.append(line);
    }
    this.parent.setAttribute('aria-activedescendant', `cell-${this.row}-${this.track}`);
    this.parent.querySelector(`#cell-${this.row}-${this.track}`)?.scrollIntoView({ block: 'nearest' });
    this.paint(this.playing);
  }

  paint(row: number | null) {
    this.playing = row;
    for (const line of this.parent.querySelectorAll<HTMLElement>('[data-row]')) line.classList.toggle('playing-row', row !== null && Number(line.dataset.row) === row);
  }

  get activeTrack() { return this.track; }

  resetHistory() { this.history.clear(); this.anchor = { row: this.row, track: this.track }; }
  undo() { if (this.history.undo(this.project().tracks)) { this.render(); this.edited(); } }
  redo() { if (this.history.redo(this.project().tracks)) { this.render(); this.edited(); } }

  private isSelected(row: number, track: number) {
    return row >= Math.min(this.row, this.anchor.row) && row <= Math.max(this.row, this.anchor.row) && track >= Math.min(this.track, this.anchor.track) && track <= Math.max(this.track, this.anchor.track);
  }

  private copy(event: ClipboardEvent) {
    const lines: string[] = [];
    for (let r = Math.min(this.row, this.anchor.row); r <= Math.max(this.row, this.anchor.row); r++) {
      const notes: string[] = [];
      for (let t = Math.min(this.track, this.anchor.track); t <= Math.max(this.track, this.anchor.track); t++) notes.push(this.project().tracks[t].notes[r] ?? '---');
      lines.push(notes.join('\t'));
    }
    event.clipboardData?.setData('text/plain', lines.join('\n')); event.preventDefault();
  }

  private clearSelection() {
    const edits: NoteEdit[] = [];
    for (let row = 0; row < 32; row++) for (let track = 0; track < 4; track++) {
      if (this.isSelected(row, track)) edits.push({ row, track, before: this.project().tracks[track].notes[row], after: null });
    }
    if (this.history.apply(this.project().tracks, edits)) { this.render(); this.edited(); }
  }

  private keydown(event: KeyboardEvent) {
    if (event.ctrlKey || event.metaKey) {
      if (event.key.toLowerCase() === 'z') { event.preventDefault(); if (event.shiftKey) this.redo(); else this.undo(); }
      if (event.key.toLowerCase() === 'a') { event.preventDefault(); this.anchor = { row: 0, track: 0 }; this.row = 31; this.track = 3; this.render(); }
      return;
    }
    if (event.altKey) return;
    switch (event.key) {
      case ' ': event.preventDefault(); this.togglePlay(); return;
      case 'ArrowUp': this.row = event.shiftKey ? Math.max(0, this.row - 1) : (this.row + 31) % 32; break;
      case 'ArrowDown': this.row = event.shiftKey ? Math.min(31, this.row + 1) : (this.row + 1) % 32; break;
      case 'ArrowLeft': this.track = event.shiftKey ? Math.max(0, this.track - 1) : (this.track + 3) % 4; break;
      case 'ArrowRight': this.track = event.shiftKey ? Math.min(3, this.track + 1) : (this.track + 1) % 4; break;
      case 'Delete': case 'Backspace': event.preventDefault(); this.clearSelection(); return;
      default: {
        const note = keyboardNote(event.key, this.octave());
        if (!note) return;
        const track = this.project().tracks[this.track];
        this.history.apply(this.project().tracks, [{ row: this.row, track: this.track, before: track.notes[this.row], after: note }]);
        if (track.instrument) this.audition(track.instrument, note, this.track);
        this.row = (this.row + 1) % 32;
        this.anchor = { row: this.row, track: this.track };
        event.preventDefault(); this.render(); this.edited(); return;
      }
    }
    if (!event.shiftKey) this.anchor = { row: this.row, track: this.track };
    event.preventDefault(); this.render();
  }
}
