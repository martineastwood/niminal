// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import { ControlInfo, Dock, format, fromPosition, groups, isGate, shortName, span, toPosition } from './controls';

const control = (over: Partial<ControlInfo> = {}): ControlInfo => ({
  name: 'drive',
  value: 0.5,
  target: 0.5,
  unit: null,
  range: [0, 1],
  log: false,
  source: null,
  kind: 'ctl',
  ...over,
});

describe('control sliders', () => {
  it('maps a value to a place on the slider and back', () => {
    const c = control({ range: [10, 20] });
    expect(toPosition(c, 15)).toBe(0.5);
    expect(fromPosition(c, 0.25)).toBe(12.5);
    expect(toPosition(c, 99)).toBe(1);
  });

  it('moves a frequency on a log scale', () => {
    const c = control({ unit: 'hz', range: [200, 3200], log: true });
    expect(toPosition(c, 800)).toBeCloseTo(0.5);
    expect(fromPosition(c, 0.5)).toBeCloseTo(800);
  });

  it('spans 0 to 1 with no range, wider if the value is outside', () => {
    expect(span(control({ range: null, value: 0.3 }))).toEqual([0, 1]);
    expect(span(control({ range: null, value: 4 }))).toEqual([0, 4]);
  });

  it('reads values with a few digits and the unit', () => {
    expect(format(control({ unit: 'hz' }), 1234.567)).toBe('1235hz');
    expect(format(control(), 0.12345)).toBe('0.123');
    expect(format(control({ unit: 'db' }), -12.34)).toBe('-12.34db');
  });

  it('groups the plain controls first, then each effect', () => {
    const list = [
      control({ name: 'g::open', kind: 'fx' }),
      control({ name: 'cutoff' }),
      control({ name: 'g::release', kind: 'fx' }),
      control({ name: '__hidden' }),
      control({ name: 'room::size', kind: 'fx' }),
    ];
    const grouped = groups(list);
    expect(grouped.map(g => [g.title, g.items.map(shortName)])).toEqual([
      [null, ['cutoff']],
      ['g', ['open', 'release']],
      ['room', ['size']],
    ]);
  });

  it('plays a control bound to a key as a held pad', () => {
    expect(isGate(control({ source: 'midi.note(36)' }))).toBe(true);
    expect(isGate(control({ source: 'midi.cc(21)' }))).toBe(false);
  });
});

describe('the dock', () => {
  const make = () => {
    const element = document.createElement('section');
    element.innerHTML = '<div class="dock-body"></div>';
    const actions = { set: vi.fn(), launch: vi.fn() };
    return { element, actions, dock: new Dock(element, actions) };
  };

  it('says so when there is nothing to show', () => {
    const { element, dock } = make();
    dock.update({ controls: [], gestures: [] });
    expect(element.querySelector('.dock-empty')?.textContent).toContain('No controls');
  });

  it('sends the value of a moved slider in the control’s own range', async () => {
    const { element, actions, dock } = make();
    dock.update({ controls: [control({ name: 'cutoff', unit: 'hz', range: [200, 3200], log: true, value: 200 })], gestures: [] });
    const input = element.querySelector<HTMLInputElement>('input[type=range]')!;
    input.value = '500';
    input.dispatchEvent(new Event('input'));
    input.value = '500';
    input.dispatchEvent(new Event('input'));
    await new Promise(done => requestAnimationFrame(() => done(null)));
    // two moves in one frame send one message, the latest
    expect(actions.set).toHaveBeenCalledOnce();
    expect(actions.set.mock.calls[0][0]).toBe('cutoff');
    expect(actions.set.mock.calls[0][1]).toBeCloseTo(800);
  });

  it('follows the engine unless the slider is being dragged', () => {
    const { element, dock } = make();
    const at = (value: number) => ({ controls: [control({ name: 'a', value })], gestures: [] });
    dock.update(at(0.2));
    const input = element.querySelector<HTMLInputElement>('input[type=range]')!;
    expect(Number(input.value)).toBe(200);
    dock.update(at(0.8));
    expect(Number(input.value)).toBe(800);
    input.dispatchEvent(new Event('pointerdown'));
    dock.update(at(0.1));
    expect(Number(input.value)).toBe(800);
    input.dispatchEvent(new Event('pointerup'));
    dock.update(at(0.1));
    expect(Number(input.value)).toBe(100);
  });

  it('starts a gesture from its pad, and holds a key control open while pressed', () => {
    const { element, actions, dock } = make();
    const clip = { name: 'roll' } as never;
    dock.update({ controls: [control({ name: 'held', source: 'midi.note(39)' })], gestures: [clip] });
    const [gesture, hold] = [...element.querySelectorAll<HTMLButtonElement>('.dock-pad')];
    gesture.click();
    expect(actions.launch).toHaveBeenCalledWith('roll');
    hold.setPointerCapture = () => {};
    hold.dispatchEvent(new Event('pointerdown'));
    expect(actions.set).toHaveBeenLastCalledWith('held', 1);
    hold.dispatchEvent(new Event('pointerup'));
    expect(actions.set).toHaveBeenLastCalledWith('held', 0);
  });
});
