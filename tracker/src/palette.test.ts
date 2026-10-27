// @vitest-environment happy-dom
import { expect, it, vi } from 'vitest';
import { filterCommands, openPalette, openShortcuts } from './palette';

const commands = (run = () => {}) => [{ label: 'Play song', keys: 'F5', run }, { label: 'Stop', run }, { label: 'Show or hide the sequence bar', run }];
const press = (target: Element, key: string) => target.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));

it('filters by every word, in any order', () => {
  expect(filterCommands(commands(), 'sequence hide').map(c => c.label)).toEqual(['Show or hide the sequence bar']);
  expect(filterCommands(commands(), '  ')).toHaveLength(3);
  expect(filterCommands(commands(), 'xyz')).toHaveLength(0);
});

it('runs the chosen command and hands focus back', () => {
  const grid = document.body.appendChild(document.createElement('button')); grid.focus();
  const ran = vi.fn();
  openPalette([{ label: 'Play song', run: () => ran('song') }, { label: 'Stop', run: () => ran('stop') }]);
  const input = document.querySelector<HTMLInputElement>('.palette input')!;
  expect(document.activeElement).toBe(input);
  press(input, 'ArrowDown'); press(input, 'Enter');
  expect(ran).toHaveBeenCalledExactlyOnceWith('stop');
  expect(document.querySelector('.palette')).toBeNull();
  grid.remove();
});

it('narrows the list as you type and closes on Escape', () => {
  openPalette(commands());
  const input = document.querySelector<HTMLInputElement>('.palette input')!;
  input.value = 'sto'; input.dispatchEvent(new Event('input'));
  expect(document.querySelectorAll('.palette-list button')).toHaveLength(1);
  press(input, 'Escape');
  expect(document.querySelector('.palette')).toBeNull();
});

it('lists the shortcuts', () => {
  openShortcuts([{ title: 'Grid', rows: [['Space', 'Play']] }]);
  expect(document.querySelector('.shortcuts-body')!.textContent).toContain('Space');
  press(document.querySelector('.shortcuts-body')!, 'Escape');
  expect(document.querySelector('.shortcuts')).toBeNull();
});
