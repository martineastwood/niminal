import { setTrackSource, trackSource, type Project, type TrackSource } from './project';
import { trackColour } from './grid';
import { openMenu } from './menu';

export interface MatrixActions {
  /** Select a sequence step in the pattern editor. */
  select(step: number): void;
  /** A cell changed. */
  edited(): void;
}

const number = (project: Project, id: string) => String(project.patterns.findIndex(p => p.id === id)).padStart(2, '0');

/**
 * The pattern matrix: a row for each sequence step and a column for each track. A cell says where the
 * track plays from in that step: its own pattern, another pattern's track, or nowhere (muted).
 */
export class MatrixPanel {
  private closeMenu = () => {};

  constructor(private parent: HTMLElement, private project: () => Project, private actions: MatrixActions) {}

  highlight(step: number) {
    for (const row of this.parent.querySelectorAll<HTMLElement>('[data-step]')) row.classList.toggle('selected', Number(row.dataset.step) === step);
  }

  paint(position: number | null) {
    for (const row of this.parent.querySelectorAll<HTMLElement>('[data-step]')) row.classList.toggle('playing', position !== null && Number(row.dataset.step) === position);
  }

  render(selected = -1) {
    this.closeMenu();
    const project = this.project();
    this.parent.replaceChildren();
    const grid = document.createElement('div'); grid.className = 'matrix-grid'; grid.style.setProperty('--tracks', String(project.tracks.length));
    const head = document.createElement('div'); head.className = 'matrix-row matrix-head'; head.append(document.createElement('span'));
    for (const [index, track] of project.tracks.entries()) {
      const name = document.createElement('span'); name.className = 'matrix-track'; name.textContent = track.name; name.title = track.name; name.style.setProperty('--track-colour', trackColour(track, index)); head.append(name);
    }
    grid.append(head);
    for (const [step, id] of project.sequence.entries()) {
      const section = project.sections?.find(s => s.start === step);
      if (section) { const label = document.createElement('div'); label.className = 'matrix-section'; label.textContent = section.name; grid.append(label); }
      const pattern = project.patterns.find(p => p.id === id)!, own = number(project, id);
      const row = document.createElement('div'); row.className = 'matrix-row'; row.dataset.step = String(step); row.classList.toggle('selected', step === selected);
      const label = document.createElement('button'); label.className = 'matrix-step'; label.textContent = `${String(step + 1).padStart(2, '0')} ${own} ${pattern.name}`; label.title = `Edit step ${step + 1}: ${pattern.name}`;
      label.setAttribute('aria-label', `Select sequence step ${step + 1}`); label.onclick = () => this.actions.select(step);
      row.append(label);
      for (const [index, track] of project.tracks.entries()) {
        const source = trackSource(project, step, index), cell = document.createElement('button'); cell.className = `matrix-cell ${source?.kind ?? 'own'}`;
        cell.textContent = source?.kind === 'mute' ? '✕' : source ? `→${number(project, source.id)}` : own;
        const what = source?.kind === 'mute' ? 'is muted' : source ? `plays pattern ${number(project, source.id)} ${project.patterns.find(p => p.id === source.id)?.name ?? ''}`.trim() : `plays its own pattern, ${own}`;
        cell.title = `${track.name} in step ${step + 1} ${what}. Click to change; Shift-click mutes or unmutes.`; cell.setAttribute('aria-label', `${track.name}, step ${step + 1}: ${what}`);
        cell.onclick = event => { if (event.shiftKey) this.set(step, index, source?.kind === 'mute' ? null : { kind: 'mute' }); else this.open(cell, step, index, id, source); };
        row.append(cell);
      }
      grid.append(row);
    }
    this.parent.append(grid);
  }

  private set(step: number, track: number, source: TrackSource | null) {
    setTrackSource(this.project(), step, track, source);
    this.actions.edited(); this.render(Number(this.parent.querySelector('.matrix-row.selected')?.getAttribute('data-step') ?? -1));
  }

  /** A menu of what the track can play in this step, under the cell. */
  private open(cell: HTMLElement, step: number, track: number, own: string, current: TrackSource | null) {
    this.closeMenu();
    const project = this.project(), label = (id: string) => `${number(project, id)} ${project.patterns.find(p => p.id === id)?.name ?? ''}`;
    const to = (source: TrackSource | null) => () => this.set(step, track, source);
    this.closeMenu = openMenu(cell, [
      { text: `Own pattern · ${label(own)}`, checked: current === null, run: to(null) },
      { text: 'Mute', checked: current?.kind === 'mute', run: to({ kind: 'mute' }) },
      { heading: 'PLAY PATTERN' },
      ...project.patterns.filter(p => p.id !== own).map(p => ({ text: `→ ${label(p.id)}`, checked: current?.kind === 'pattern' && current.id === p.id, run: to({ kind: 'pattern', id: p.id }) })),
    ]);
  }
}
