export interface NoteEdit { track: number; row: number; before: string | null; after: string | null }

export function parseNotes(text: string): (string | null)[][] | null {
  if (!text.trim()) return null;
  const lines = text.replace(/\r/g, '').trim().split('\n');
  if (lines.length > 256) return null;
  const rows = lines.map(line => line.split('\t').map(value => value.trim()));
  if (rows.some(row => row.length > 80 || row.some(value => !['', '---', '···', '··'].includes(value) && !/^(?:[A-G][#-][0-8]|OFF|CUT|C|[A-Z][0-9A-F]{2}|[0-9A-F]{4})$/i.test(value) && !/^[-+]?\d*\.?\d+(?:e[-+]?\d+)?\s*(?:khz|hz|ms|sec|s|db|%|st|deg)?$/i.test(value)))) return null;
  return rows.map(row => row.map(value => ['', '---', '···', '··'].includes(value) ? null : value.toUpperCase()));
}

/** One chronological history for the complete saved project, including definition source. */
export class ProjectHistory {
  private past: string[] = [];
  private future: string[] = [];
  private group: string | undefined;
  private recordedAt = 0;
  constructor(private current: string) {}
  get canUndo() { return this.past.length > 0; }
  get canRedo() { return this.future.length > 0; }
  record(value: string, group?: string, now = Date.now()) {
    if (value === this.current) return;
    if (!group || group !== this.group || now - this.recordedAt > 750) this.past.push(this.current);
    this.group = group; this.recordedAt = now;
    if (this.past.length > 100) this.past.shift();
    this.current = value; this.future = [];
  }
  undo(): string | undefined {
    this.group = undefined;
    const value = this.past.pop(); if (value === undefined) return;
    this.future.push(this.current); return this.current = value;
  }
  redo(): string | undefined {
    this.group = undefined;
    const value = this.future.pop(); if (value === undefined) return;
    this.past.push(this.current); return this.current = value;
  }
  reset(value: string) { this.group = undefined; this.current = value; this.past = []; this.future = []; }
}
