import { isTauri } from '@tauri-apps/api/core';

/** How the window is laid out and sized: kept between sessions, separately from the project. */
export interface ViewPrefs {
  /** Zoom of the whole interface */
  scale: number;
  /** Font size of the code editor, in px */
  codeFont: number;
  /** The sequence bar and the right bar (project instruments and sound browser) are shown */
  sequence: boolean;
  side: boolean;
  sequenceWidth: number;
  sideWidth: number;
  /** Brighter text and borders */
  contrast: boolean;
}

export const scales = [0.85, 1, 1.15, 1.3, 1.5, 1.75, 2];
export const fontRange = { min: 9, max: 32 };
export const defaultView: ViewPrefs = { scale: 1, codeFont: 12, sequence: true, side: true, sequenceWidth: 168, sideWidth: 220, contrast: false };
export const widthRange = { min: 120, max: 520 };
const storageKey = 'niminal-tracker-view';

const clamp = (value: number, min: number, max: number) => Math.max(min, Math.min(max, value));
/** The scale `delta` steps from `scale` (the nearest step when it is between two), stopping at the ends. */
export function stepScale(scale: number, delta: number) {
  let nearest = 0;
  for (const [i, s] of scales.entries()) if (Math.abs(s - scale) < Math.abs(scales[nearest] - scale)) nearest = i;
  return scales[clamp(nearest + delta, 0, scales.length - 1)];
}
export const clampFont = (px: number) => clamp(Math.round(px), fontRange.min, fontRange.max);
export const clampWidth = (px: number) => clamp(Math.round(px), widthRange.min, widthRange.max);

/** Saved preferences, with anything missing or out of range replaced by its default. */
export function readView(raw: string | null): ViewPrefs {
  let saved: Partial<ViewPrefs> = {};
  try { const parsed = raw ? JSON.parse(raw) : {}; if (parsed && typeof parsed === 'object') saved = parsed; } catch { /* use the defaults */ }
  const number = (value: unknown, fallback: number) => typeof value === 'number' && Number.isFinite(value) ? value : fallback;
  const flag = (value: unknown, fallback: boolean) => typeof value === 'boolean' ? value : fallback;
  return {
    scale: stepScale(number(saved.scale, defaultView.scale), 0),
    codeFont: clampFont(number(saved.codeFont, defaultView.codeFont)),
    sequence: flag(saved.sequence, defaultView.sequence),
    side: flag(saved.side, defaultView.side),
    sequenceWidth: clampWidth(number(saved.sequenceWidth, defaultView.sequenceWidth)),
    sideWidth: clampWidth(number(saved.sideWidth, defaultView.sideWidth)),
    contrast: flag(saved.contrast, defaultView.contrast),
  };
}
export function loadView(): ViewPrefs {
  try { return readView(localStorage.getItem(storageKey)); } catch { return { ...defaultView }; }
}
export function saveView(prefs: ViewPrefs) {
  try { localStorage.setItem(storageKey, JSON.stringify(prefs)); } catch { /* preferences just aren't kept */ }
}

/** Zooms the whole window: natively in the app, so pointer positions stay right, and with CSS in a browser. */
async function applyZoom(scale: number) {
  if (isTauri()) {
    try { const { getCurrentWebview } = await import('@tauri-apps/api/webview'); await getCurrentWebview().setZoom(scale); return; }
    catch { /* fall back to CSS */ }
  }
  document.documentElement.style.zoom = String(scale);
}

/** Puts `prefs` on the page. `focus` hides everything but the grid, and is not kept. */
export function applyView(prefs: ViewPrefs, focus = false) {
  const root = document.documentElement;
  root.style.setProperty('--code-font', `${prefs.codeFont}px`);
  root.style.setProperty('--sequence-width', `${prefs.sequenceWidth}px`);
  root.style.setProperty('--side-width', `${prefs.sideWidth}px`);
  root.classList.toggle('hide-sequence', !prefs.sequence);
  root.classList.toggle('hide-side', !prefs.side);
  root.classList.toggle('focus-mode', focus);
  root.classList.toggle('high-contrast', prefs.contrast);
  void applyZoom(prefs.scale);
}

/** Lets the pointer drag `handle` to size a side bar: `sign` is 1 when growing means moving right. */
export function dragWidth(handle: HTMLElement, sign: 1 | -1, current: () => number, set: (width: number) => void, done: () => void) {
  handle.onpointerdown = event => {
    const startX = event.clientX, startWidth = current();
    handle.setPointerCapture(event.pointerId);
    handle.onpointermove = move => set(clampWidth(startWidth + sign * (move.clientX - startX)));
    handle.onpointerup = () => { handle.onpointermove = null; handle.onpointerup = null; done(); };
  };
  // the arrow keys size it too
  handle.onkeydown = event => {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    event.preventDefault(); set(clampWidth(current() + sign * (event.key === 'ArrowRight' ? 1 : -1) * (event.shiftKey ? 40 : 10))); done();
  };
}
