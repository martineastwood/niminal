/** One thing the browser can pick: a preset, or an effect already in the project. */
export interface BrowserItem { value: string; name: string; folder: string; description?: string; /** Small text at the end of the row, such as how many tracks use it. */ note?: string }

interface Folder { name: string; folders: Map<string, Folder>; items: BrowserItem[] }

/** The items as a tree of folders, split at `/`. */
export function folderTree(items: BrowserItem[]): Folder {
  const root: Folder = { name: '', folders: new Map(), items: [] };
  for (const item of items) {
    let at = root;
    for (const part of item.folder.split('/').filter(Boolean)) {
      if (!at.folders.has(part)) at.folders.set(part, { name: part, folders: new Map(), items: [] });
      at = at.folders.get(part)!;
    }
    at.items.push(item);
  }
  return root;
}

const count = (folder: Folder): number => folder.items.length + [...folder.folders.values()].reduce((sum, f) => sum + count(f), 0);

/** The items whose name, folder or description has every word of `query`, in their given order. */
export function searchItems(items: BrowserItem[], query: string): BrowserItem[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return items.filter(item => { const text = `${item.folder} ${item.name} ${item.description ?? ''}`.toLowerCase(); return words.every(word => text.includes(word)); });
}

/**
 * A file browser under `anchor`: folders that open and close, with the presets in them, and a search
 * box that lists the matches from every folder. It closes when something is picked, on a click
 * elsewhere, or on Escape (which puts focus back on the anchor). Returns a function that closes it.
 */
export function openBrowser(anchor: HTMLElement, items: BrowserItem[], pick: (item: BrowserItem) => void): () => void {
  const panel = document.createElement('div'); panel.className = 'browser'; panel.setAttribute('role', 'dialog'); panel.setAttribute('aria-label', 'Browse presets');
  const search = document.createElement('input'); search.type = 'search'; search.placeholder = 'Search'; search.setAttribute('aria-label', 'Search presets');
  const list = document.createElement('div'); list.className = 'browser-list';
  const hint = document.createElement('div'); hint.className = 'browser-hint';
  const outside = (event: Event) => { if (!panel.contains(event.target as Node)) close(); };
  const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') { event.stopPropagation(); close(); anchor.focus(); } };
  const close = () => { panel.remove(); document.removeEventListener('pointerdown', outside, true); document.removeEventListener('keydown', escape, true); };
  const entry = (item: BrowserItem, path = '') => {
    const button = document.createElement('button'); button.className = 'browser-item'; button.textContent = item.name; button.title = item.description ?? '';
    if (path) { const where = document.createElement('span'); where.className = 'browser-path'; where.textContent = path; button.append(where); }
    const describe = () => { hint.textContent = item.description ?? ''; };
    button.onmouseenter = describe; button.onfocus = describe; button.onclick = () => { close(); pick(item); };
    return button;
  };
  const folderElement = (folder: Folder): HTMLElement => {
    const details = document.createElement('details'); details.className = 'browser-folder';
    const summary = document.createElement('summary'); summary.textContent = `${folder.name} (${count(folder)})`; details.append(summary);
    for (const sub of folder.folders.values()) details.append(folderElement(sub));
    for (const item of folder.items) details.append(entry(item));
    return details;
  };
  const show = () => {
    list.replaceChildren(); hint.textContent = '';
    if (search.value.trim()) {
      const found = searchItems(items, search.value);
      if (!found.length) { const none = document.createElement('div'); none.className = 'browser-none'; none.textContent = 'Nothing matches'; list.append(none); }
      for (const item of found) list.append(entry(item, item.folder));
      return;
    }
    const tree = folderTree(items);
    for (const folder of tree.folders.values()) list.append(folderElement(folder));
    for (const item of tree.items) list.append(entry(item));
  };
  search.oninput = show;
  search.onkeydown = event => { if (event.key === 'Enter') { const first = list.querySelector<HTMLElement>('.browser-item'); if (first && search.value.trim()) first.click(); } };
  panel.append(search, list, hint); show();
  document.body.append(panel);
  const box = anchor.getBoundingClientRect();
  panel.style.left = `${Math.max(4, Math.min(box.left, innerWidth - panel.offsetWidth - 4))}px`;
  panel.style.top = `${box.bottom + panel.offsetHeight + 4 > innerHeight ? Math.max(4, box.top - panel.offsetHeight) : box.bottom}px`;
  document.addEventListener('pointerdown', outside, true); document.addEventListener('keydown', escape, true);
  search.focus();
  return close;
}
