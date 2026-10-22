export interface NoteEdit { track: number; row: number; before: string | null; after: string | null }

export class NoteHistory {
  private past: NoteEdit[][] = [];
  private future: NoteEdit[][] = [];

  apply(tracks: (string | null)[][], edits: NoteEdit[]) {
    edits = edits.filter(edit => edit.before !== edit.after);
    if (!edits.length) return false;
    for (const edit of edits) tracks[edit.track][edit.row] = edit.after;
    this.past.push(edits); if (this.past.length > 500) this.past.shift();
    this.future = [];
    return true;
  }
  undo(tracks: (string | null)[][]) {
    const edits = this.past.pop(); if (!edits) return false;
    for (const edit of edits) tracks[edit.track][edit.row] = edit.before;
    this.future.push(edits); return true;
  }
  redo(tracks: (string | null)[][]) {
    const edits = this.future.pop(); if (!edits) return false;
    for (const edit of edits) tracks[edit.track][edit.row] = edit.after;
    this.past.push(edits); return true;
  }
  clear() { this.past = []; this.future = []; }
}

export function parseNotes(text: string): (string | null)[][] | null {
  if (!text.trim()) return null;
  const lines = text.replace(/\r/g, '').trim().split('\n');
  if (lines.length > 256) return null;
  const rows = lines.map(line => line.split('\t').map(value => value.trim()));
  if (rows.some(row => row.length > 16 || row.some(value => !['', '---', '···'].includes(value) && !/^[A-G][#-][0-8]$/i.test(value)))) return null;
  return rows.map(row => row.map(value => ['', '---', '···'].includes(value) ? null : value.toUpperCase()));
}
