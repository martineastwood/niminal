/** A line of a menu: an item, a heading, or a number to choose with a button to use it. */
export type MenuEntry = { text: string; checked?: boolean; run: () => void } | { heading: string } | { number: string; value: number; min: number; max: number; run: (value: number) => void };

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
    if ('number' in entry) {
      const row = document.createElement('div'); row.className = 'menu-number';
      const label = document.createElement('label'); label.textContent = `${entry.number} `;
      const input = document.createElement('input'); input.type = 'number'; input.min = String(entry.min); input.max = String(entry.max); input.value = String(entry.value); input.setAttribute('aria-label', entry.number);
      const go = document.createElement('button'); go.textContent = 'Go';
      const use = () => { const value = Math.trunc(input.valueAsNumber); if (value >= entry.min && value <= entry.max) { close(); entry.run(value); } else input.focus(); };
      go.onclick = use; input.onkeydown = event => { if (event.key === 'Enter') { event.preventDefault(); use(); } };
      label.append(input); row.append(label, go); menu.append(row); continue;
    }
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
