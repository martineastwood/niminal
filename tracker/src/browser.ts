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

/** The items whose name, folder or description has every word of `query`, in their given order. */
export function searchItems(items: BrowserItem[], query: string): BrowserItem[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return items.filter(item => { const text = `${item.folder} ${item.name} ${item.description ?? ''}`.toLowerCase(); return words.every(word => text.includes(word)); });
}
