export type MenuEntry = { text: string; checked?: boolean; run: () => void } | { heading: string };

/**
 * A menu under `anchor`. It closes when an item is chosen, on a click elsewhere, or on Escape (which
 * puts focus back on the anchor). Returns a function that closes it.
 */
export function openMenu(anchor: HTMLElement, entries: MenuEntry[]): () => void {
  const menu = document.createElement('div'); menu.className = 'menu'; menu.setAttribute('role', 'menu');
  const outside = (event: Event) => { if (!menu.contains(event.target as Node)) close(); };
  const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') { event.stopPropagation(); close(); anchor.focus(); } };
  const close = () => { menu.remove(); document.removeEventListener('pointerdown', outside, true); document.removeEventListener('keydown', escape, true); };
  for (const entry of entries) {
    if ('heading' in entry) { const heading = document.createElement('div'); heading.className = 'menu-heading'; heading.textContent = entry.heading; menu.append(heading); continue; }
    const button = document.createElement('button'); button.textContent = entry.text; button.setAttribute('role', 'menuitemradio'); button.setAttribute('aria-checked', String(!!entry.checked));
    button.onclick = () => { close(); entry.run(); }; menu.append(button);
  }
  document.body.append(menu);
  const box = anchor.getBoundingClientRect();
  menu.style.left = `${Math.max(4, Math.min(box.left, innerWidth - menu.offsetWidth - 4))}px`;
  menu.style.top = `${box.bottom + menu.offsetHeight + 4 > innerHeight ? Math.max(4, box.top - menu.offsetHeight) : box.bottom}px`;
  document.addEventListener('pointerdown', outside, true); document.addEventListener('keydown', escape, true);
  (menu.querySelector<HTMLElement>('[aria-checked=true]') ?? menu.querySelector<HTMLElement>('button'))?.focus();
  return close;
}
