/** Something the command palette can run; `keys` is only shown, as a reminder. */
export interface Command { label: string; keys?: string; run: () => void }
export interface ShortcutSection { title: string; rows: [keys: string, what: string][] }

/** The commands whose label holds every word of `query`, in their order. */
export function filterCommands(commands: Command[], query: string): Command[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return commands.filter(command => words.every(word => command.label.toLowerCase().includes(word)));
}

/** Opens a modal over the page; it closes on Escape or a click outside, and gives focus back to what had it. */
function modal(className: string, label: string) {
  const previous = document.activeElement as HTMLElement | null;
  const backdrop = document.createElement('div'); backdrop.className = 'modal-backdrop';
  const box = document.createElement('div'); box.className = className; box.setAttribute('role', 'dialog'); box.setAttribute('aria-label', label);
  backdrop.append(box);
  const close = (restore = true) => { backdrop.remove(); if (restore) previous?.focus(); };
  backdrop.onpointerdown = event => { if (event.target === backdrop) close(); };
  box.onkeydown = event => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); } };
  document.body.append(backdrop);
  return { box, close };
}

/** The command palette: type to narrow the list, arrows to choose, Enter to run. */
export function openPalette(commands: Command[]) {
  if (document.querySelector('.palette')) return;
  const { box, close } = modal('palette', 'Command palette');
  const input = document.createElement('input'); input.type = 'search'; input.placeholder = 'Type a command…'; input.setAttribute('aria-label', 'Command');
  const list = document.createElement('div'); list.className = 'palette-list'; list.setAttribute('role', 'listbox');
  box.append(input, list);
  let shown: Command[] = [], at = 0;
  const run = (command: Command | undefined) => { if (!command) return; close(); command.run(); };
  const render = () => {
    shown = filterCommands(commands, input.value); at = Math.min(at, Math.max(0, shown.length - 1)); list.replaceChildren();
    if (!shown.length) { const none = document.createElement('div'); none.className = 'palette-none'; none.textContent = 'No matching command'; list.append(none); return; }
    for (const [i, command] of shown.entries()) {
      const row = document.createElement('button'); row.setAttribute('role', 'option'); row.setAttribute('aria-selected', String(i === at)); row.tabIndex = -1;
      const name = document.createElement('span'); name.textContent = command.label; row.append(name);
      if (command.keys) { const keys = document.createElement('kbd'); keys.textContent = command.keys; row.append(keys); }
      row.onclick = () => run(command);
      list.append(row);
    }
    list.querySelector('[aria-selected=true]')?.scrollIntoView?.({ block: 'nearest' });
  };
  input.oninput = () => { at = 0; render(); };
  input.onkeydown = event => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); at = shown.length ? (at + (event.key === 'ArrowDown' ? 1 : shown.length - 1)) % shown.length : 0; render(); }
    else if (event.key === 'Enter') { event.preventDefault(); run(shown[at]); }
  };
  render(); input.focus();
}

/** A reference of the keys, grouped by what they work on. */
export function openShortcuts(sections: ShortcutSection[]) {
  if (document.querySelector('.shortcuts')) return;
  const { box, close } = modal('shortcuts', 'Keyboard shortcuts');
  const heading = document.createElement('div'); heading.className = 'shortcuts-heading';
  const title = document.createElement('strong'); title.textContent = 'Keyboard shortcuts';
  const done = document.createElement('button'); done.textContent = 'Close (Esc)'; done.onclick = () => close();
  heading.append(title, done); box.append(heading);
  const body = document.createElement('div'); body.className = 'shortcuts-body'; body.tabIndex = 0; box.append(body);
  for (const section of sections) {
    const group = document.createElement('section'); const name = document.createElement('h3'); name.textContent = section.title; group.append(name);
    for (const [keys, what] of section.rows) {
      const row = document.createElement('div'); const key = document.createElement('kbd'); key.textContent = keys;
      const text = document.createElement('span'); text.textContent = what; row.append(key, text); group.append(row);
    }
    body.append(group);
  }
  body.focus();
}
