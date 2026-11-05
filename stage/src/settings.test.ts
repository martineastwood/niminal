// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { settingsPage } from './settings';

describe('the settings page', () => {
  it('starts closed, opens and closes, and holds the Look settings', () => {
    const page = settingsPage();
    document.body.append(page.element);
    expect(page.isOpen).toBe(false);
    page.toggle();
    expect(page.isOpen).toBe(true);
    expect(page.element.querySelector('#look-preset')).not.toBeNull();
    page.toggle();
    expect(page.isOpen).toBe(false);
    page.element.remove();
  });

  it('closes from its button and from a click outside the card, but not from a click inside it', () => {
    const page = settingsPage();
    document.body.append(page.element);
    page.toggle();
    page.element.querySelector('.settings-card')!.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    expect(page.isOpen).toBe(true);
    page.element.dispatchEvent(new MouseEvent('mousedown'));
    expect(page.isOpen).toBe(false);
    page.toggle();
    page.element.querySelector<HTMLElement>('#settings-close')!.click();
    expect(page.isOpen).toBe(false);
    page.element.remove();
  });
});
