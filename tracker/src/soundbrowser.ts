import { folderTree, searchItems, type BrowserItem } from './browser';

export type SoundKind = 'instr' | 'fx';

/** What the buttons under the list do. `new` and `import` need no selection; the rest act on the selected sound of the project. */
export type ManageOp = 'new' | 'import' | 'duplicate' | 'delete' | 'edit';

export interface SoundBrowserActions {
  /** Hears an item: called as the selection moves, after a short pause so scrolling past does not play everything. */
  audition(kind: SoundKind, item: BrowserItem): void;
  /** Uses an item (Enter or double-click). `replace` (Shift+Enter) loads a ready-made instrument into the current slot instead of adding a slot. */
  pick(kind: SoundKind, item: BrowserItem, replace: boolean): void;
  /** One of the buttons under the list, with the selected item (if any). */
  manage(op: ManageOp, kind: SoundKind, item: BrowserItem | null): void;
  /** The browser lost focus or was dismissed: stop any preview. */
  leave(): void;
}

const pause = 120;

type Row = { folder: string; name: string; depth: number } | { item: BrowserItem; depth: number; path: string };

/**
 * A browser that stays open beside the grid: instruments and effects in folders, a search box, and the
 * arrow keys to move through them. Moving the selection plays the item; Enter adds it to the selected track.
 */
export class SoundBrowser {
  private kind: SoundKind = 'instr';
  private query = '';
  private open = new Map<string, boolean>();
  /** The folder the cursor is on, when it is on one rather than on an item. */
  private folderCursor: string | null = null;
  private selected: BrowserItem | null = null;
  private timer: number | undefined;
  private readonly list = document.createElement('div');
  private readonly search = document.createElement('input');
  private readonly hint = document.createElement('div');
  private readonly target = document.createElement('div');
  private readonly tabs = new Map<SoundKind, HTMLButtonElement>();
  private readonly tools = document.createElement('div');

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
    const tools = this.tools; tools.className = 'sb-tools';
    for (const [op, text, title] of [['new', '+', 'New blank sound'], ['duplicate', '⧉', 'Duplicate the selected sound of this project'], ['delete', '−', 'Delete the selected sound of this project'], ['edit', '{}', 'Edit the code of the selected sound of this project']] as const) {
      const button = document.createElement('button'); button.textContent = text; button.title = title; button.setAttribute('aria-label', title);
      button.onclick = () => actions.manage(op, this.kind, this.selected);
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
    this.kind = kind; this.selected = null; this.folderCursor = null; this.hint.textContent = ''; this.query = ''; this.search.value = '';
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

  /** The folder `key` (like `/drums/kicks`) is open: all start closed, except the project's own sounds. */
  private isOpen(key: string) { return this.open.get(key) ?? key === '/Project'; }

  /** What the list shows, top to bottom: folders and the items inside the open ones, or only the matches of a search. */
  private rows(): Row[] {
    const items = this.items(this.kind);
    if (this.query.trim()) return searchItems(items, this.query).map(item => ({ item, depth: 0, path: item.folder }));
    const rows: Row[] = [];
    const walk = (folder: ReturnType<typeof folderTree>, path: string, depth: number) => {
      for (const [name, sub] of folder.folders) {
        const key = `${path}/${name}`;
        rows.push({ folder: key, name, depth });
        if (this.isOpen(key)) walk(sub, key, depth + 1);
      }
      for (const item of folder.items) rows.push({ item, depth, path: '' });
    };
    walk(folderTree(items), '', 0);
    return rows;
  }

  private toggle(key: string, open = !this.isOpen(key)) { this.open.set(key, open); this.render(); }

  private render() {
    for (const [kind, tab] of this.tabs) tab.setAttribute('aria-selected', String(kind === this.kind));
    // the project's instruments have a panel of their own, with its own buttons
    this.tools.hidden = this.kind === 'instr';
    const top = this.list.scrollTop;
    this.list.replaceChildren();
    const rows = this.rows();
    if (!rows.length) { const none = document.createElement('div'); none.className = 'sb-none'; none.textContent = 'Nothing matches'; this.list.append(none); }
    for (const row of rows) {
      if ('folder' in row) {
        const open = this.isOpen(row.folder);
        const head = document.createElement('button'); head.className = 'sb-folder'; head.tabIndex = -1; head.textContent = `${open ? '▾' : '▸'} ${row.name}`;
        head.setAttribute('aria-selected', String(row.folder === this.folderCursor)); head.style.paddingLeft = `${4 + row.depth * 10}px`;
        head.onclick = () => { this.folderCursor = row.folder; this.selected = null; this.hint.textContent = ''; this.toggle(row.folder); this.search.focus(); };
        this.list.append(head);
        continue;
      }
      const { item } = row;
      const button = document.createElement('button'); button.className = 'sb-item'; button.tabIndex = -1; button.setAttribute('role', 'option');
      button.setAttribute('aria-selected', String(item.value === this.selected?.value)); button.textContent = item.name; button.title = item.description ?? '';
      button.style.paddingLeft = `${4 + row.depth * 10}px`;
      if (row.path || item.note) { const where = document.createElement('span'); where.className = 'browser-path'; where.textContent = row.path || item.note!; button.append(where); }
      button.onclick = () => { this.select(item, true); this.search.focus(); };
      button.ondblclick = () => this.actions.pick(this.kind, item, false);
      this.list.append(button);
    }
    this.list.scrollTop = top;
    this.list.querySelector('[aria-selected=true]')?.scrollIntoView({ block: 'nearest' });
  }

  private select(item: BrowserItem, immediately = false) {
    this.selected = item; this.folderCursor = null; this.hint.textContent = item.description ?? '';
    this.render();
    clearTimeout(this.timer);
    this.timer = window.setTimeout(() => this.actions.audition(this.kind, item), immediately ? 0 : pause);
  }

  /** Moves the cursor over folders and items alike; landing on an item plays it, landing on a folder does not. */
  private move(delta: number) {
    const rows = this.rows();
    if (!rows.length) return;
    const at = rows.findIndex(row => 'folder' in row ? row.folder === this.folderCursor : row.item.value === this.selected?.value);
    const row = rows[Math.min(rows.length - 1, Math.max(0, at + delta))];
    if ('item' in row) { this.select(row.item); return; }
    clearTimeout(this.timer);
    this.folderCursor = row.folder; this.selected = null; this.hint.textContent = '';
    this.render();
  }

  private keydown(event: KeyboardEvent, focusGrid: () => void) {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    if (event.key === 'ArrowDown') { event.preventDefault(); this.move(1); }
    else if (event.key === 'ArrowUp') { event.preventDefault(); this.move(-1); }
    else if (event.key === 'Enter') {
      event.preventDefault();
      if (this.folderCursor) this.toggle(this.folderCursor);
      else if (this.selected) this.actions.pick(this.kind, this.selected, event.shiftKey);
      else this.move(1);
    }
    else if ((event.key === 'ArrowRight' || event.key === 'ArrowLeft') && this.folderCursor && !this.query.trim()) { event.preventDefault(); this.toggle(this.folderCursor, event.key === 'ArrowRight'); }
    else if (event.key === ' ' && this.selected && (event.target as HTMLElement).tagName !== 'INPUT') { event.preventDefault(); this.select(this.selected, true); }
    else if (event.key === 'Escape') { event.preventDefault(); clearTimeout(this.timer); this.actions.leave(); focusGrid(); }
  }
}
