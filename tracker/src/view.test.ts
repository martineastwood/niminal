// @vitest-environment happy-dom
import { expect, it, vi } from 'vitest';
import { applyView, clampFont, clampWidth, defaultView, dragWidth, readView, stepScale } from './view';

vi.mock('@tauri-apps/api/core', () => ({ isTauri: () => false }));

it('steps the scale to the neighbouring size and stops at the ends', () => {
  expect(stepScale(1, 1)).toBe(1.15);
  expect(stepScale(1, -1)).toBe(0.85);
  expect(stepScale(2, 1)).toBe(2);
  expect(stepScale(0.85, -1)).toBe(0.85);
  expect(stepScale(1.2, 0)).toBe(1.15);
});

it('replaces missing, broken or out-of-range preferences with defaults', () => {
  expect(readView(null)).toEqual(defaultView);
  expect(readView('not json')).toEqual(defaultView);
  expect(readView('{"scale":"big","codeFont":500,"sequence":false,"sideWidth":1}')).toEqual({ ...defaultView, codeFont: 32, sequence: false, sideWidth: 120 });
  expect(clampFont(3)).toBe(9);
  expect(clampWidth(9000)).toBe(520);
});

it('puts the preferences on the page', () => {
  applyView({ ...defaultView, scale: 1.3, codeFont: 18, sequence: false, contrast: true }, true);
  const root = document.documentElement;
  expect(root.style.getPropertyValue('--code-font')).toBe('18px');
  expect(root.classList.contains('hide-sequence')).toBe(true);
  expect(root.classList.contains('hide-side')).toBe(false);
  expect(root.classList.contains('focus-mode')).toBe(true);
  expect(root.classList.contains('high-contrast')).toBe(true);
  expect(root.style.zoom).toBe('1.3');
});

it('sizes a side bar from the keyboard', () => {
  const handle = document.createElement('div'); let width = 200, done = 0;
  dragWidth(handle, -1, () => width, next => { width = next; }, () => { done++; });
  handle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft' }));
  expect(width).toBe(210); // the right bar grows as its edge moves left
  handle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', shiftKey: true }));
  expect(width).toBe(170);
  expect(done).toBe(2);
});
