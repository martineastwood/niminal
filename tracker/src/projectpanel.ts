import type { ManageOp } from './soundbrowser';

export interface ProjectInstrument { id: string; name: string; /** Small text at the end of the row, such as how many tracks use it. */ note?: string }

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
  /** The buttons above the list. */
  manage(op: ManageOp): void;
  /** The scissors button: offers to cut the current sampler into pieces, with a menu under `anchor`. */
  slice(anchor: HTMLElement): void;
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
    const tools = document.createElement('span'); tools.className = 'side-tools';
    for (const [op, text, label] of [['new', '+', 'New instrument'], ['duplicate', '⧉', 'Duplicate the current instrument'], ['delete', '−', 'Delete the current instrument'], ['import', '♪', 'New sampler from WAV files']] as const) {
      const button = document.createElement('button'); button.textContent = text; button.title = label; button.setAttribute('aria-label', label);
      button.onclick = () => actions.manage(op); tools.append(button);
    }
    const scissors = document.createElement('button'); scissors.textContent = '✂'; scissors.title = 'New instrument that plays the current sampler cut into pieces'; scissors.setAttribute('aria-label', scissors.title);
    scissors.onclick = () => actions.slice(scissors); tools.append(scissors);
    heading.append(tools);
    this.list.className = 'pp-list'; this.list.tabIndex = 0; this.list.setAttribute('role', 'listbox'); this.list.setAttribute('aria-label', 'Project instruments');
    this.list.onkeydown = event => this.keydown(event);
    root.append(heading, this.list);
    this.refresh();
  }

  focus() { this.list.focus(); }

  /** Draws the instruments again, keeping the scroll position unless the current one is out of view. */
  refresh() {
    const top = this.list.scrollTop, current = this.current();
    this.list.replaceChildren();
    for (const [index, item] of this.items().entries()) {
      const row = document.createElement('button'); row.className = 'pp-item'; row.tabIndex = -1; row.setAttribute('role', 'option');
      row.setAttribute('aria-selected', String(item.id === current)); row.textContent = `${String(index).padStart(2, '0')}  ${item.name}`; row.title = item.name;
      if (item.note) { const note = document.createElement('small'); note.textContent = item.note; row.append(note); }
      row.onclick = () => { this.actions.choose(item.id); this.list.focus(); };
      row.ondblclick = () => this.actions.edit(item.id);
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
    else if (event.key === 'Escape') { event.preventDefault(); this.actions.leave(); }
  }
}
