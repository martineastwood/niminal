import { setTrackSource, trackSource, type Project, type TrackSource } from './project';
import { trackColour } from './grid';
import { openMenu } from './menu';

const number = (project: Project, id: string) => String(project.patterns.findIndex(p => p.id === id)).padStart(2, '0');

let closeMenu = () => {};
/** Closes the menu a cell opened, before the sequence is drawn again. */
export const closeMatrixMenu = () => closeMenu();

/**
 * One cell of the pattern matrix, shown on a sequence step's row: where a track plays from in that
 * step. Blank is its own pattern, a number is another pattern's track, and ✕ is muted.
 */
export function matrixCell(project: Project, step: number, track: number, changed: () => void): HTMLButtonElement {
  const own = project.sequence[step], source = trackSource(project, step, track), name = project.tracks[track].name;
  const cell = document.createElement('button'); cell.className = `matrix-cell ${source?.kind ?? 'own'}`;
  cell.style.setProperty('--track-colour', trackColour(project.tracks[track], track));
  cell.textContent = source?.kind === 'mute' ? '✕' : source ? number(project, source.id) : '';
  const what = source?.kind === 'mute' ? 'is muted' : source ? `plays pattern ${number(project, source.id)} ${project.patterns.find(p => p.id === source.id)?.name ?? ''}`.trim() : `plays its own pattern, ${number(project, own)}`;
  cell.title = `${name} in step ${step + 1} ${what}. Click to change; Shift-click mutes or unmutes.`; cell.setAttribute('aria-label', `${name}, step ${step + 1}: ${what}`);
  const set = (to: TrackSource | null) => { setTrackSource(project, step, track, to); changed(); };
  cell.onclick = event => {
    if (event.shiftKey) { set(source?.kind === 'mute' ? null : { kind: 'mute' }); return; }
    closeMenu();
    const label = (id: string) => `${number(project, id)} ${project.patterns.find(p => p.id === id)?.name ?? ''}`;
    closeMenu = openMenu(cell, [
      { text: `Own pattern · ${label(own)}`, checked: source === null, run: () => set(null) },
      { text: 'Mute', checked: source?.kind === 'mute', run: () => set({ kind: 'mute' }) },
      { heading: 'PLAY PATTERN' },
      ...project.patterns.filter(p => p.id !== own).map(p => ({ text: `→ ${label(p.id)}`, checked: source?.kind === 'pattern' && source.id === p.id, run: () => set({ kind: 'pattern', id: p.id }) })),
    ]);
  };
  return cell;
}
