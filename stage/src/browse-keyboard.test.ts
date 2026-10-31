// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { FileBrowse } from './browse';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.body.replaceChildren();
  localStorage.clear();
});

it('navigates and auditions samples after clicking a file', async () => {
  const files = ['kick.wav', 'snare.wav'].map(name => `/set/${name}`);
  vi.mocked(invoke).mockImplementation(async command => command === 'list_files' ? files : new ArrayBuffer(0));
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null);
  vi.spyOn(HTMLElement.prototype, 'scrollIntoView').mockImplementation(() => {});
  const start = vi.fn();
  vi.stubGlobal('AudioContext', class {
    destination = {};
    async decodeAudioData() {
      return { getChannelData: () => new Float32Array(), duration: 1, numberOfChannels: 1, sampleRate: 44100 };
    }
    createBufferSource() { return { connect() {}, start, stop() {} }; }
  });
  const browser = new FileBrowse({ fileDir: () => '/set', insert: vi.fn(), openPiece: vi.fn(), status: vi.fn() });
  document.body.append(browser.element);
  browser.toggle();
  await vi.waitFor(() => expect(browser.element.querySelectorAll('.entry')).toHaveLength(2));
  const list = browser.element.querySelector<HTMLElement>('.files-list')!;
  list.querySelector<HTMLElement>('.entry')!.click();
  expect(document.activeElement).toBe(list);
  await vi.waitFor(() => expect(start).toHaveBeenCalledTimes(1));
  const key = (key: string) => document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
  key('ArrowDown');
  expect(list.querySelector('.on')!.textContent).toBe('snare.wav');
  await vi.waitFor(() => expect(start).toHaveBeenCalledTimes(2));
  expect(invoke).toHaveBeenLastCalledWith('read_sample', { path: '/set/snare.wav' });
  key('ArrowUp');
  expect(list.querySelector('.on')!.textContent).toBe('kick.wav');
  await vi.waitFor(() => expect(start).toHaveBeenCalledTimes(3));
  key('Escape');
  expect(browser.open).toBe(false);
});

it('enters expanded folders with Right and returns to parents with Left', async () => {
  vi.mocked(invoke).mockResolvedValue([
    '/set/drums/hats/open.nml',
    '/set/drums/kick.nml',
    '/set/main.nml',
  ]);
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null);
  vi.spyOn(HTMLElement.prototype, 'scrollIntoView').mockImplementation(() => {});
  const browser = new FileBrowse({ fileDir: () => '/set', insert: vi.fn(), openPiece: vi.fn(), status: vi.fn() });
  document.body.append(browser.element);
  browser.toggle();
  await vi.waitFor(() => expect(browser.element.querySelectorAll('.entry')).toHaveLength(2));
  const list = browser.element.querySelector<HTMLElement>('.files-list')!;
  list.querySelector<HTMLElement>('.entry')!.click();
  const key = (key: string) => document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
  const selected = () => list.querySelector('.on')!.textContent;
  expect(selected()).toBe('▾ drums');
  key('ArrowRight'); expect(selected()).toBe('▸ hats');
  key('ArrowRight'); expect(selected()).toBe('▾ hats');
  key('ArrowRight'); expect(selected()).toBe('open.nml');
  key('ArrowLeft'); expect(selected()).toBe('▾ hats');
  key('ArrowLeft'); expect(selected()).toBe('▸ hats');
  key('ArrowLeft'); expect(selected()).toBe('▾ drums');
  key('ArrowLeft'); expect(selected()).toBe('▸ drums');
  expect(document.activeElement).toBe(list);
  expect(list.querySelectorAll('.entry')).toHaveLength(2);
  browser.close();
});

it('browses above the chosen root with Left and the Up button', async () => {
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null);
  vi.spyOn(HTMLElement.prototype, 'scrollIntoView').mockImplementation(() => {});
  vi.mocked(invoke).mockImplementation(async (_command, args) => {
    const dir = (args as { dir: string }).dir;
    return dir === '/music/set' ? ['/music/set/main.nml']
      : dir === '/music' ? ['/music/set/main.nml', '/music/other/beat.nml']
      : [];
  });
  const browser = new FileBrowse({ fileDir: () => '/music/set', insert: vi.fn(), openPiece: vi.fn(), status: vi.fn() });
  document.body.append(browser.element);
  browser.toggle();
  await vi.waitFor(() => expect(browser.element.querySelectorAll('.entry')).toHaveLength(1));
  const list = browser.element.querySelector<HTMLElement>('.files-list')!;
  list.querySelector<HTMLElement>('.entry')!.click();
  list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true, cancelable: true }));
  await vi.waitFor(() => expect(list.querySelector('.on')?.textContent).toBe('▸ set'));
  expect(browser.element.querySelector('.files-where')!.textContent).toBe('/music');
  expect(localStorage.getItem('niminal-stage:samples')).toBe('/music');
  expect(document.activeElement).toBe(list);
  const up = browser.element.querySelector<HTMLButtonElement>('.files-up')!;
  up.click();
  await vi.waitFor(() => expect(browser.element.querySelector('.files-detail')!.textContent).toBe('no pieces or audio in this folder'));
  expect(browser.element.querySelector('.files-where')!.textContent).toBe('/');
  expect(up.disabled).toBe(true);
  browser.close();
});

it('drags the browser wider, remembers its width, and resets it', () => {
  const options = { fileDir: () => '/set', insert: vi.fn(), openPiece: vi.fn(), status: vi.fn() };
  const browser = new FileBrowse(options);
  const parent = document.createElement('div');
  Object.defineProperty(parent, 'clientWidth', { value: 1000 });
  parent.append(browser.element);
  document.body.append(parent);
  vi.spyOn(browser.element, 'getBoundingClientRect').mockReturnValue({ width: 260 } as DOMRect);
  const divider = browser.element.querySelector<HTMLElement>('.files-divider')!;
  let captured = false;
  divider.setPointerCapture = () => { captured = true; };
  divider.hasPointerCapture = () => captured;
  divider.releasePointerCapture = () => { captured = false; };
  divider.dispatchEvent(new PointerEvent('pointerdown', { clientX: 260, button: 0, pointerId: 1 }));
  divider.dispatchEvent(new PointerEvent('pointermove', { clientX: 500, pointerId: 1 }));
  expect(browser.element.style.getPropertyValue('--browser-width')).toBe('500px');
  divider.dispatchEvent(new PointerEvent('pointermove', { clientX: 2000, pointerId: 1 }));
  expect(browser.element.style.getPropertyValue('--browser-width')).toBe('760px');
  divider.dispatchEvent(new PointerEvent('pointerup', { pointerId: 1 }));
  expect(document.body.classList.contains('resizing')).toBe(false);
  expect(new FileBrowse(options).element.style.getPropertyValue('--browser-width')).toBe('760px');
  divider.dispatchEvent(new MouseEvent('dblclick'));
  expect(browser.element.style.getPropertyValue('--browser-width')).toBe('');
  expect(localStorage.getItem('niminal-stage:browser-width')).toBeNull();
});
