import { folderTree, searchItems, type BrowserItem } from './browser';

export type SoundKind = 'instr' | 'fx';

/** What the buttons under the list do. `new` and `import` need no selection; the rest act on the selected sound of the project. */
export type ManageOp = 'new' | 'import' | 'duplicate' | 'delete' | 'edit';

export interface SoundBrowserActions {
  /** Hears an item: called as the selection moves, after a short pause so scrolling past does not play everything. */
  audition(kind: SoundKind, item: BrowserItem): void;
  /** Uses an item on the selected track (Enter or double-click). */
  pick(kind: SoundKind, item: BrowserItem): void;
  /** One of the buttons under the list, with the selected item (if any). */
  manage(op: ManageOp, kind: SoundKind, item: BrowserItem | null): void;
  /** The browser lost focus or was dismissed: stop any preview. */
  leave(): void;
}

const pause = 120;

/**
 * A browser that stays open beside the grid: instruments and effects in folders, a search box, and the
 * arrow keys to move through them. Moving the selection plays the item; Enter adds it to the selected track.
 */
export class SoundBrowser {
  private kind: SoundKind = 'instr';
  private query = '';
  private closed = new Set<string>();
  private selected: BrowserItem | null = null;
  private timer: number | undefined;
  private readonly list = document.createElement('div');
  private readonly search = document.createElement('input');
  private readonly hint = document.createElement('div');
  private readonly target = document.createElement('div');
  private readonly tabs = new Map<SoundKind, HTMLButtonElement>();
  private importButton = document.createElement('button');

  /** `items` gives what each tab lists, asked again on `refresh` so it follows the project. */
  constructor(private readonly root: HTMLElement, private readonly items: (kind: SoundKind) => BrowserItem[], private readonly actions: SoundBrowserActions, focusGrid: () => void) {
    const bar = document.createElement('div'); bar.className = 'sb-tabs';
    for (const [kind, label] of [['instr', 'Instruments'], ['fx', 'Effects']] as const) {
      const tab = document.createElement('button'); tab.textContent = label; tab.onclick = () => this.show(kind); this.tabs.set(kind, tab); bar.append(tab);
    }
    this.search.type = 'search'; this.search.placeholder = 'Search'; this.search.setAttribute('aria-label', 'Search sounds');
    this.search.oninput = () => { this.query = this.search.value; this.render(); };
    this.list.className = 'sb-list'; this.list.tabIndex = 0; this.list.setAttribute('role', 'listbox');
    this.hint.className = 'sb-hint'; this.target.className = 'sb-target';
    const tools = document.createElement('div'); tools.className = 'sb-tools';
    for (const [op, text, title] of [['new', '+', 'New blank sound'], ['duplicate', '⧉', 'Duplicate the selected sound of this project'], ['delete', '−', 'Delete the selected sound of this project'], ['edit', '{}', 'Edit the code of the selected sound of this project'], ['import', '♪', 'New sampler from WAV files']] as const) {
      const button = document.createElement('button'); button.textContent = text; button.title = title; button.setAttribute('aria-label', title);
      button.onclick = () => actions.manage(op, this.kind, this.selected);
      if (op === 'import') this.importButton = button;
      tools.append(button);
    }
    root.append(bar, this.search, this.list, tools, this.hint, this.target);
    root.addEventListener('keydown', event => this.keydown(event, focusGrid));
    root.addEventListener('focusout', event => { if (!root.contains(event.relatedTarget as Node | null)) { clearTimeout(this.timer); actions.leave(); } });
    this.render();
  }

  /** Says where Enter will put the selected item. */
  setTarget(text: string) { this.target.textContent = text; }

  /** Switches tab and puts the cursor in the search box, ready to type or move down. */
  show(kind: SoundKind, focus = true) {
    this.kind = kind; this.selected = null; this.hint.textContent = ''; this.query = ''; this.search.value = '';
    this.render();
    if (focus) this.search.focus();
  }

  /** Lists the project's sounds again, keeping the selection if it is still there. */
  refresh() {
    const value = this.selected?.value;
    this.selected = value ? this.items(this.kind).find(item => item.value === value) ?? null : null;
    this.render();
  }

  focus() { this.search.focus(); this.search.select(); }

  private visible(): BrowserItem[] {
    const items = this.items(this.kind);
    if (this.query.trim()) return searchItems(items, this.query);
    const flat: BrowserItem[] = [];
    const walk = (folder: ReturnType<typeof folderTree>, path: string) => {
      for (const [name, sub] of folder.folders) if (!this.closed.has(`${path}/${name}`)) walk(sub, `${path}/${name}`);
      flat.push(...folder.items);
    };
    walk(folderTree(items), '');
    return flat;
  }

  private render() {
    for (const [kind, tab] of this.tabs) tab.setAttribute('aria-selected', String(kind === this.kind));
    this.importButton.hidden = this.kind !== 'instr';
    const top = this.list.scrollTop;
    this.list.replaceChildren();
    const entry = (item: BrowserItem, path = '') => {
      const button = document.createElement('button'); button.className = 'sb-item'; button.tabIndex = -1; button.setAttribute('role', 'option');
      button.setAttribute('aria-selected', String(item.value === this.selected?.value)); button.textContent = item.name; button.title = item.description ?? '';
      if (path || item.note) { const where = document.createElement('span'); where.className = 'browser-path'; where.textContent = path || item.note!; button.append(where); }
      button.onclick = () => { this.select(item, true); this.search.focus(); };
      button.ondblclick = () => this.actions.pick(this.kind, item);
      return button;
    };
    if (this.query.trim()) {
      const found = searchItems(this.items(this.kind), this.query);
      if (!found.length) { const none = document.createElement('div'); none.className = 'sb-none'; none.textContent = 'Nothing matches'; this.list.append(none); }
      for (const item of found) this.list.append(entry(item, item.folder));
    } else {
      const draw = (folder: ReturnType<typeof folderTree>, path: string, parent: HTMLElement) => {
        for (const [name, sub] of folder.folders) {
          const key = `${path}/${name}`, collapsed = this.closed.has(key);
          const head = document.createElement('button'); head.className = 'sb-folder'; head.tabIndex = -1; head.textContent = `${collapsed ? '▸' : '▾'} ${name}`;
          head.onclick = () => { if (collapsed) this.closed.delete(key); else this.closed.add(key); this.render(); };
          parent.append(head);
          if (!collapsed) { const inner = document.createElement('div'); inner.className = 'sb-inner'; parent.append(inner); draw(sub, key, inner); }
        }
        for (const item of folder.items) parent.append(entry(item));
      };
      draw(folderTree(this.items(this.kind)), '', this.list);
    }
    this.list.scrollTop = top;
    this.list.querySelector('.sb-item[aria-selected=true]')?.scrollIntoView({ block: 'nearest' });
  }

  private select(item: BrowserItem, immediately = false) {
    this.selected = item; this.hint.textContent = item.description ?? '';
    this.render();
    clearTimeout(this.timer);
    this.timer = window.setTimeout(() => this.actions.audition(this.kind, item), immediately ? 0 : pause);
  }

  private move(delta: number) {
    const items = this.visible();
    if (!items.length) return;
    const at = this.selected ? items.findIndex(item => item.value === this.selected!.value) : -1;
    this.select(items[Math.min(items.length - 1, Math.max(0, at + delta))]);
  }

  private keydown(event: KeyboardEvent, focusGrid: () => void) {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    if (event.key === 'ArrowDown') { event.preventDefault(); this.move(1); }
    else if (event.key === 'ArrowUp') { event.preventDefault(); this.move(-1); }
    else if (event.key === 'Enter') { event.preventDefault(); if (this.selected) this.actions.pick(this.kind, this.selected); else this.move(1); }
    else if (event.key === ' ' && this.selected && (event.target as HTMLElement).tagName !== 'INPUT') { event.preventDefault(); this.select(this.selected, true); }
    else if (event.key === 'Escape') { event.preventDefault(); clearTimeout(this.timer); this.actions.leave(); focusGrid(); }
  }
}
