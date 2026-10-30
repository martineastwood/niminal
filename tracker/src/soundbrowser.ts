import { folderTree, searchItems, type BrowserItem } from './browser';
import { draggable } from './drag';

export type SoundKind = 'instr' | 'sample' | 'fx';

/** What the buttons under the list do. */
export type ManageOp = 'folder' | 'import' | 'kit';


export interface SoundBrowserActions {
  /** Hears an item: called as the selection moves, after a short pause so scrolling past does not play everything. */
  audition(kind: SoundKind, item: BrowserItem): void;
  /** Uses an item (Enter or double-click). `replace` (Shift+Enter or Shift+double-click) puts it in the current slot instead of adding a slot. */
  pick(kind: SoundKind, item: BrowserItem, replace: boolean): void;
  /** The "⋯" button of a row, or a right-click on it: opens a menu of what can be done to the item, under `anchor`. */
  menu(kind: SoundKind, item: BrowserItem, anchor: HTMLElement): void;
  /** Several samples at once (Cmd/Ctrl-clicked rows, or the samples directly in a folder): opens a menu of what can be done with them, under `anchor`. */
  group(items: BrowserItem[], anchor: HTMLElement): void;
  /** One of the buttons under the list. */
  manage(op: ManageOp, kind: SoundKind): void;
  /** Stop any preview: the browser lost focus, was dismissed, or the playing sample was clicked again. */
  leave(): void;
}

const pause = 120;

type Row = { folder: string; name: string; depth: number } | { item: BrowserItem; depth: number; path: string };

const favouritesKey = 'niminal-tracker-favourites';
const labels: [SoundKind, string][] = [['instr', 'Instruments'], ['sample', 'Samples'], ['fx', 'Effects']];
const buttons: Record<SoundKind, [ManageOp, string, string][]> = {
  instr: [],
  sample: [['folder', 'Choose folder…', 'Browse the audio files in a folder on disk'], ['import', 'Import files…', 'Add audio files to the project, one sampler each'], ['kit', 'Import as kit…', 'Add audio files as one instrument with a sample on each key']],
  fx: [],
};

/**
 * A browser that stays open beside the grid: instruments, samples and effects in folders, a search box, and the
 * arrow keys to move through them. Moving the selection plays the item; Enter adds it to the selected track.
 * Rows can be starred (they are then also listed under Favourites at the top) and dragged onto the slots above.
 */
export class SoundBrowser {
  private kind: SoundKind = 'instr';
  private query = '';
  private open = new Map<string, boolean>();
  /** The folder the cursor is on, when it is on one rather than on an item. */
  private folderCursor: string | null = null;
  private selected: BrowserItem | null = null;
  /** Samples picked together with Cmd/Ctrl-click or Shift-click, to be used as a group. */
  private readonly marked = new Set<string>();
  private timer: number | undefined;
  private readonly list = document.createElement('div');
  private readonly search = document.createElement('input');
  private readonly hint = document.createElement('div');
  private readonly target = document.createElement('div');
  private readonly tabs = new Map<SoundKind, HTMLButtonElement>();
  private readonly tools = document.createElement('div');
  private readonly favourites = new Set<string>();

  /** `items` gives what each tab lists, asked again on `refresh` so it follows the project. */
  constructor(private readonly root: HTMLElement, private readonly items: (kind: SoundKind) => BrowserItem[], private readonly actions: SoundBrowserActions, focusGrid: () => void, only?: SoundKind) {
    if (only) this.kind = only;
    const bar = document.createElement('div'); bar.className = 'sb-tabs';
    for (const [kind, label] of labels) {
      if (only || kind === 'fx') continue; // effects are added from the dock's own list
      const tab = document.createElement('button'); tab.textContent = label; tab.onclick = () => this.show(kind); this.tabs.set(kind, tab); bar.append(tab);
    }
    this.search.type = 'search'; this.search.placeholder = 'Search'; this.search.setAttribute('aria-label', 'Search sounds');
    this.search.oninput = () => { this.query = this.search.value; this.render(); };
    this.list.className = 'sb-list'; this.list.tabIndex = 0; this.list.setAttribute('role', 'listbox');
    this.hint.className = 'sb-hint'; this.target.className = 'sb-target';
    this.tools.className = 'sb-tools';
    try { for (const key of JSON.parse(localStorage.getItem(favouritesKey) ?? '[]') as string[]) this.favourites.add(key); } catch { /* no favourites kept */ }
    root.append(...only ? [] : [bar], this.search, this.list, this.tools, ...only ? [] : [this.hint, this.target]); // the effects list has neither: a row's tooltip says what it is, and the chain says where it goes
    root.addEventListener('keydown', event => this.keydown(event, focusGrid));
    root.addEventListener('focusout', event => { if (!root.contains(event.relatedTarget as Node | null)) { clearTimeout(this.timer); actions.leave(); } });
    this.render();
  }

  /** Says where Enter will put the selected item. */
  setTarget(text: string) { this.target.textContent = text; }

  /** Switches tab and puts the cursor in the search box, ready to type or move down. */
  show(kind: SoundKind, focus = true) {
    this.kind = kind; this.selected = null; this.marked.clear(); this.folderCursor = null; this.hint.textContent = ''; this.query = ''; this.search.value = '';
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

  /** The folder `key` (like `/drums/kicks`) is open: all start closed, except Favourites. */
  private isOpen(key: string) { return this.open.get(key) ?? key === '/Favourites'; }

  /** What the list shows, top to bottom: folders and the items inside the open ones, or only the matches of a search. */
  private rows(): Row[] {
    const items = this.items(this.kind);
    if (this.query.trim()) return searchItems(items, this.query).map(item => ({ item, depth: 0, path: item.folder }));
    const rows: Row[] = [];
    const starred = items.filter(item => this.favourites.has(this.starKey(item)));
    if (starred.length) { rows.push({ folder: '/Favourites', name: 'Favourites', depth: 0 }); if (this.isOpen('/Favourites')) for (const item of starred) rows.push({ item, depth: 1, path: '' }); }
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

  /** What the favourites remember an item by: kept by tab, since an instrument and an effect may share an id. Items of the project itself are not kept. */
  private starKey(item: BrowserItem) { return item.value.startsWith('project:') ? '' : `${this.kind}:${item.value}`; }

  private toggleStar(item: BrowserItem) {
    const key = this.starKey(item);
    if (!key) return;
    try { this.favourites.clear(); for (const kept of JSON.parse(localStorage.getItem(favouritesKey) ?? '[]') as string[]) this.favourites.add(kept); } catch { /* keep what is here */ } // another browser may have starred something
    if (!this.favourites.delete(key)) this.favourites.add(key);
    try { localStorage.setItem(favouritesKey, JSON.stringify([...this.favourites])); } catch { /* not kept */ }
    this.render();
  }

  private render() {
    for (const [kind, tab] of this.tabs) tab.setAttribute('aria-selected', String(kind === this.kind));
    this.tools.replaceChildren();
    for (const [op, text, title] of buttons[this.kind]) {
      const button = document.createElement('button'); button.textContent = text; button.title = title; button.onclick = () => this.actions.manage(op, this.kind); this.tools.append(button);
    }
    this.tools.hidden = !buttons[this.kind].length;
    const top = this.list.scrollTop;
    this.list.replaceChildren();
    const rows = this.rows();
    if (!rows.length) { const none = document.createElement('div'); none.className = 'sb-none'; none.textContent = this.kind === 'sample' && !this.query.trim() ? 'Choose a folder of samples below, or drop audio files here' : 'Nothing matches'; this.list.append(none); }
    for (const row of rows) {
      if ('folder' in row) {
        const open = this.isOpen(row.folder);
        const head = document.createElement('button'); head.className = 'sb-folder'; head.tabIndex = -1; head.textContent = `${open ? '▾' : '▸'} ${row.name}`;
        head.setAttribute('aria-selected', String(row.folder === this.folderCursor)); head.style.paddingLeft = `${6 + row.depth * 12}px`;
        head.oncontextmenu = event => {
          const inside = this.items(this.kind).filter(item => `/${item.folder}` === row.folder);
          if (this.kind === 'sample' && inside.length) { event.preventDefault(); this.actions.group(inside, head); }
        };
        head.onclick = () => { this.folderCursor = row.folder; this.selected = null; this.hint.textContent = ''; this.toggle(row.folder); this.search.focus(); };
        this.list.append(head);
        continue;
      }
      const { item } = row;
      const button = document.createElement('div'); button.className = 'sb-item'; button.tabIndex = -1; button.setAttribute('role', 'option');
      button.setAttribute('aria-selected', String(this.marked.size ? this.marked.has(item.value) : item.value === this.selected?.value)); button.title = item.description ?? '';
      button.style.paddingLeft = `${6 + row.depth * 12}px`;
      const name = document.createElement('span'); name.className = 'sb-name'; name.textContent = item.name; button.append(name);
      if ((row.path || item.note) && this.kind !== 'sample') { const where = document.createElement('span'); where.className = 'browser-path'; where.textContent = row.path || item.note!; button.append(where); }
      const key = this.starKey(item);
      if (key) {
        const star = document.createElement('button'); star.className = 'sb-star'; star.tabIndex = -1; star.dataset.noDrag = ''; star.setAttribute('aria-label', 'Favourite'); star.setAttribute('aria-pressed', String(this.favourites.has(key)));
        star.onclick = event => { event.stopPropagation(); this.toggleStar(item); };
        button.append(star);
      }
      const more = document.createElement('button'); more.className = 'sb-more'; more.tabIndex = -1; more.dataset.noDrag = ''; more.setAttribute('aria-label', 'More actions');
      // the menu opens before the row is selected: selecting draws the list again, which would take the anchor out of the page
      more.onclick = event => { event.stopPropagation(); this.menuFor(item, more); };
      button.append(more);
      button.onclick = event => { this.click(item, event); this.search.focus(); };
      button.ondblclick = event => this.actions.pick(this.kind, item, event.shiftKey);
      button.oncontextmenu = event => { event.preventDefault(); this.menuFor(item, button); };
      if (this.kind !== 'fx') draggable(button, () => ({ kind: this.kind as 'instr' | 'sample', value: item.value, name: item.name }));
      this.list.append(button);
    }
    this.list.scrollTop = top;
    this.list.querySelector('[aria-selected=true]')?.scrollIntoView?.({ block: 'nearest' });
  }

  /** The marked samples, in the order they are listed. */
  private markedItems() { return this.items(this.kind).filter(item => this.marked.has(item.value)); }

  /** A click on a sample row: Cmd/Ctrl adds or removes it from the group, Shift marks the rows from the selected one to it, a plain click selects (and plays) just it. */
  private click(item: BrowserItem, event: MouseEvent) {
    if (this.kind !== 'sample' || !(event.metaKey || event.ctrlKey || event.shiftKey)) { this.select(item, true); return; }
    clearTimeout(this.timer);
    if (event.shiftKey && this.selected) {
      const shown = this.rows().flatMap(row => 'item' in row ? [row.item.value] : []);
      const [from, to] = [shown.indexOf(this.selected.value), shown.indexOf(item.value)].sort((a, b) => a - b);
      if (from >= 0) { this.marked.clear(); for (const value of shown.slice(from, to + 1)) this.marked.add(value); this.render(); return; }
    }
    if (!this.marked.size && this.selected) this.marked.add(this.selected.value);
    if (!this.marked.delete(item.value)) this.marked.add(item.value);
    this.selected = item; this.folderCursor = null; this.hint.textContent = item.description ?? '';
    this.render();
  }

  /** The "⋯" button or right-click of a row: the group's menu when the row is one of several marked, else the row's own. */
  private menuFor(item: BrowserItem, anchor: HTMLElement) {
    if (this.marked.size > 1 && this.marked.has(item.value)) { this.actions.group(this.markedItems(), anchor); return; }
    this.actions.menu(this.kind, item, anchor); this.select(item, true);
  }

  private select(item: BrowserItem, immediately = false) {
    this.marked.clear();
    this.selected = item; this.folderCursor = null; this.hint.textContent = item.description ?? '';
    this.render();
    clearTimeout(this.timer);
    // samples can be long loops, so moving over them stays silent: click or Space plays (and stops it again), Esc stops
    if (this.kind === 'sample' && !immediately) return;
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
      else if (this.marked.size > 1) this.actions.group(this.markedItems(), this.list);
      else if (this.selected) this.actions.pick(this.kind, this.selected, event.shiftKey);
      else this.move(1);
    }
    else if ((event.key === 'ArrowRight' || event.key === 'ArrowLeft') && this.folderCursor && !this.query.trim()) { event.preventDefault(); this.toggle(this.folderCursor, event.key === 'ArrowRight'); }
    else if (event.key === ' ' && this.selected && (event.target as HTMLElement).tagName !== 'INPUT') { event.preventDefault(); this.select(this.selected, true); }
    else if (event.key === 'Escape') { event.preventDefault(); this.marked.clear(); clearTimeout(this.timer); this.actions.leave(); focusGrid(); }
  }
}
