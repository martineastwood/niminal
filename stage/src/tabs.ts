/** What is remembered of the open tabs between runs: where each came from, its text, and the text last saved. */
export type StoredTab = { path: string | null; title: string; text: string; saved: string };
export type StoredTabs = { tabs: StoredTab[]; active: number };

export const tabsKey = 'niminal-stage:tabs';

export function tabTitle(path: string | null, fallback: string) {
  return path ? (path.split(/[\\/]/).pop() ?? path) : fallback;
}

export function loadTabs(raw: string | null): StoredTabs | null {
  try {
    const parsed = JSON.parse(raw ?? 'null') as StoredTabs | null;
    if (!parsed || !Array.isArray(parsed.tabs) || parsed.tabs.length === 0) return null;
    const active = Math.min(Math.max(Number(parsed.active) || 0, 0), parsed.tabs.length - 1);
    return { tabs: parsed.tabs, active };
  } catch {
    return null;
  }
}

/** Move the item at `from` so it sits before the item that was at `before` (the end if `before` is the length). */
export function moveItem<T>(items: T[], from: number, before: number) {
  const [item] = items.splice(from, 1);
  items.splice(before > from ? before - 1 : before, 0, item);
}
