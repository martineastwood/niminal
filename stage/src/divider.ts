/**
 * Let `handle` size a panel: drag it, or use the arrow keys (Shift for bigger steps), and double-click to reset.
 * `sign` is 1 when the panel grows as the handle moves right, -1 when it grows as it moves left.
 */
export function divider(handle: HTMLElement, sign: 1 | -1, width: () => number, set: (width: number) => void, reset: () => void) {
  let startX = 0;
  let startWidth = 0;
  handle.addEventListener('pointerdown', event => {
    if (event.button !== 0) return;
    // without this the drag also selects the text and controls it passes over
    event.preventDefault();
    startX = event.clientX;
    startWidth = width();
    handle.setPointerCapture(event.pointerId);
    handle.classList.add('dragging');
    document.body.classList.add('resizing');
  });
  handle.addEventListener('pointermove', event => {
    if (handle.hasPointerCapture(event.pointerId)) set(startWidth + sign * (event.clientX - startX));
  });
  const end = () => {
    handle.classList.remove('dragging');
    document.body.classList.remove('resizing');
  };
  for (const name of ['pointerup', 'pointercancel', 'lostpointercapture']) handle.addEventListener(name, end);
  handle.addEventListener('dblclick', reset);
  handle.addEventListener('keydown', event => {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    event.preventDefault();
    set(width() + sign * (event.key === 'ArrowRight' ? 1 : -1) * (event.shiftKey ? 80 : 20));
  });
}
