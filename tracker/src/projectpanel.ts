import { dropTarget, type Dragged } from './drag';

export interface ProjectInstrument { id: string; name: string; /** How a sampler plays, shown as a small tag, with what it means as its tooltip. */ tag?: { text: string; title: string }; /** Small text at the end of the row, such as how many tracks use it. */ note?: string }

/** The instrument `delta` places from `current` in the list, stopping at the ends; null when there is none to move to. */
export function neighbour(ids: string[], current: string | null, delta: number): string | null {
  const next = ids[Math.min(ids.length - 1, Math.max(0, ids.indexOf(current ?? '') + delta))];
  return next !== undefined && next !== current ? next : null;
}

export interface ProjectPanelActions {
  /** Makes an instrument the current one (a click, or the arrow keys on the list). */
  choose(id: string): void;
  /** Opens an instrument's code (a double-click, or Enter). */
  edit(id: string): void;
  /** The Add button: a menu of ways to make an instrument, under `anchor`. */
  add(anchor: HTMLElement): void;
  /** A row's "⋯" button or a right-click on it: a menu of what can be done to that instrument, under `anchor`. */
  menu(id: string, anchor: HTMLElement): void;
  /** Delete or Backspace on the list: removes the instrument. */
  remove(id: string): void;
  /** A row of the sound browser was dropped on an instrument (replacing it), or on the empty space (adding it). */
  drop(item: Dragged, id: string | null): void;
  /** Escape: the cursor goes back to the grid. */
  leave(): void;
}

/**
 * The project's instruments, always in view: numbered as the Ins column counts them, with the current one
 * highlighted and kept on screen. The list scrolls when there are more than fit.
 */
export class ProjectPanel {
  private readonly list = document.createElement('div');

  constructor(private readonly root: HTMLElement, private readonly items: () => ProjectInstrument[], private readonly current: () => string | null, private readonly actions: ProjectPanelActions) {
    const heading = document.createElement('div'); heading.className = 'pp-heading';
    const title = document.createElement('span'); title.className = 'section-title'; title.textContent = 'INSTRUMENTS'; heading.append(title);
    const add = document.createElement('button'); add.className = 'pp-add'; add.textContent = '+ Add'; add.title = 'Add an instrument: blank, from audio files, or cut from a sampler';
    add.onclick = () => actions.add(add); heading.append(add);
    this.list.className = 'pp-list'; this.list.tabIndex = 0; this.list.setAttribute('role', 'listbox'); this.list.setAttribute('aria-label', 'Project instruments');
    this.list.onkeydown = event => this.keydown(event);
    const droppable = (item: Dragged) => item.kind !== 'fx';
    dropTarget(this.list, droppable, item => actions.drop(item, null));
    root.append(heading, this.list);
    this.refresh();
  }

  focus() { this.list.focus(); }

  /** Draws the instruments again, keeping the scroll position unless the current one is out of view. */
  refresh() {
    const top = this.list.scrollTop, current = this.current();
    this.list.replaceChildren();
    const actions = this.actions;
    for (const [index, item] of this.items().entries()) {
      const row = document.createElement('div'); row.className = 'pp-item'; row.tabIndex = -1; row.setAttribute('role', 'option');
      row.setAttribute('aria-selected', String(item.id === current)); row.title = item.name;
      const name = document.createElement('span'); name.className = 'pp-name'; name.textContent = `${String(index).padStart(2, '0')}  ${item.name}`; row.append(name);
      if (item.tag) { const tag = document.createElement('span'); tag.className = 'pp-tag'; tag.textContent = item.tag.text; tag.title = item.tag.title; row.append(tag); }
      if (item.note) { const note = document.createElement('small'); note.textContent = item.note; row.append(note); }
      const more = document.createElement('button'); more.className = 'pp-more'; more.tabIndex = -1; more.setAttribute('aria-label', `More actions for ${item.name}`);
      // the menu opens before the instrument is chosen: choosing draws the list again, which would take the anchor out of the page
      more.onclick = event => { event.stopPropagation(); this.actions.menu(item.id, more); this.actions.choose(item.id); };
      row.append(more);
      row.onclick = () => { this.actions.choose(item.id); this.list.focus(); };
      row.ondblclick = () => this.actions.edit(item.id);
      row.oncontextmenu = event => { event.preventDefault(); this.actions.menu(item.id, row); this.actions.choose(item.id); };
      dropTarget(row, dragged => dragged.kind !== 'fx', dragged => actions.drop(dragged, item.id));
      this.list.append(row);
    }
    this.list.scrollTop = top;
    (this.list.querySelector('[aria-selected=true]') as HTMLElement | null)?.scrollIntoView?.({ block: 'nearest' });
  }

  private keydown(event: KeyboardEvent) {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const ids = this.items().map(item => item.id), current = this.current();
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const next = neighbour(ids, current, event.key === 'ArrowDown' ? 1 : -1);
      if (next) this.actions.choose(next);
    } else if (event.key === 'Enter' && current && ids.includes(current)) { event.preventDefault(); this.actions.edit(current); }
    else if ((event.key === 'Delete' || event.key === 'Backspace') && current && ids.includes(current)) { event.preventDefault(); this.actions.remove(current); }
    else if (event.key === 'Escape') { event.preventDefault(); this.actions.leave(); }
  }
}
