// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from 'vitest';
import { PRESETS, look, loadLook, lookSection, presetOf, setLook } from './look';

beforeEach(() => {
  localStorage.clear();
  setLook(PRESETS.standard);
});

describe('the look of the window', () => {
  it('knows a preset by its values, and calls anything else custom', () => {
    setLook(PRESETS.video);
    expect(presetOf()).toBe('video');
    setLook({ glow: 1.2 });
    expect(presetOf()).toBe('custom');
  });

  it('keeps settings within their range', () => {
    setLook({ glow: 99, trail: -3, fps: 15 as 30 });
    expect(look).toMatchObject({ glow: 2.5, trail: 0.2, fps: 30 });
  });

  it('remembers what was chosen, and falls back to standard when what was kept is unreadable', () => {
    setLook(PRESETS.video);
    setLook(PRESETS.calm);
    loadLook();
    expect(presetOf()).toBe('calm');
    localStorage.setItem('niminal-stage-look', '{not json');
    loadLook();
    expect(presetOf()).toBe('standard');
  });

  it('turns the large transport on with the video preset, and off again', () => {
    setLook(PRESETS.video);
    expect(document.body.classList.contains('large-transport')).toBe(true);
    setLook(PRESETS.standard);
    expect(document.body.classList.contains('large-transport')).toBe(false);
  });

  it('sets the glow scale the stylesheet reads', () => {
    setLook({ glow: 1.5 });
    expect(document.documentElement.style.getPropertyValue('--glow')).toBe('1.5');
  });

  it('shows a preset in the panel, and follows a change made in it', () => {
    const panel = lookSection();
    document.body.append(panel);
    const preset = panel.querySelector<HTMLSelectElement>('#look-preset')!;
    preset.value = 'video';
    preset.dispatchEvent(new Event('change'));
    expect(look.fps).toBe(60);
    const glow = panel.querySelector<HTMLInputElement>('#look-glow')!;
    glow.value = '0.3';
    glow.dispatchEvent(new Event('input'));
    expect(preset.value).toBe('custom');
    panel.remove();
  });
});
