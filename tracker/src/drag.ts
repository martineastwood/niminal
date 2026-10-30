/** What a row of the sound browser carries when it is dragged. */
export interface Dragged { kind: 'instr' | 'sample' | 'fx' | 'track' | 'step'; value: string; name: string }

const threshold = 5;
const targets = new WeakMap<Element, { accept: (item: Dragged) => boolean; drop: (item: Dragged, target: Element) => void }>();

/** Makes `element` somewhere a dragged row can be dropped: `accept` says which, `drop` receives it. A drop lands on the innermost target that accepts it. */
export function dropTarget(element: Element, accept: (item: Dragged) => boolean, drop: (item: Dragged, target: Element) => void) {
  targets.set(element, { accept, drop });
}

const targetAt = (x: number, y: number, item: Dragged) => {
  for (let at = document.elementFromPoint(x, y); at; at = at.parentElement) { if (targets.get(at)?.accept(item)) return at; }
  return null;
};

/**
 * Lets a pointer drag `element` onto a drop target. Moving a few pixels starts the drag, with a label that follows the
 * pointer and the target under it marked `drop-over`; letting go drops it, and the click that would follow is swallowed.
 * (Pointer events rather than HTML5 drag and drop, which the desktop window takes over for files dropped from outside.)
 */
export function draggable(element: HTMLElement, item: () => Dragged) {
  element.addEventListener('pointerdown', down => {
    if (down.button !== 0 || (down.target as HTMLElement).closest('[data-no-drag]')) return;
    const carried = item();
    let ghost: HTMLElement | null = null, over: Element | null = null;
    const move = (event: PointerEvent) => {
      if (!ghost) {
        if (Math.hypot(event.clientX - down.clientX, event.clientY - down.clientY) < threshold) return;
        ghost = document.createElement('div'); ghost.className = 'drag-ghost'; ghost.textContent = carried.name; document.body.append(ghost);
        document.body.classList.add('dragging');
      }
      ghost.style.left = `${event.clientX + 10}px`; ghost.style.top = `${event.clientY + 6}px`;
      const next = targetAt(event.clientX, event.clientY, carried);
      if (next !== over) { over?.classList.remove('drop-over'); over = next; over?.classList.add('drop-over'); }
    };
    const end = (event: PointerEvent) => {
      document.removeEventListener('pointermove', move, true); document.removeEventListener('pointerup', end, true); document.removeEventListener('pointercancel', end, true);
      const dragged = ghost !== null, target = event.type === 'pointerup' ? over : null;
      ghost?.remove(); document.body.classList.remove('dragging'); over?.classList.remove('drop-over');
      if (!dragged) return;
      const swallow = (click: Event) => { click.stopPropagation(); click.preventDefault(); };
      document.addEventListener('click', swallow, true); setTimeout(() => document.removeEventListener('click', swallow, true));
      if (target) targets.get(target)?.drop(carried, target);
    };
    document.addEventListener('pointermove', move, true); document.addEventListener('pointerup', end, true); document.addEventListener('pointercancel', end, true);
  });
}
