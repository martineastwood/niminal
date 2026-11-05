import { lookSection } from './look';

/** The settings page: a card over the window, opened from the menu bar (or Cmd/Ctrl+,) and closed with Escape,
 *  the close button, or a click outside it. Each kind of setting is a section in it. */
export function settingsPage(extra: { sections?: HTMLElement[]; onOpen?: () => void } = {}) {
  const page = document.createElement('div');
  page.id = 'settings';
  page.className = 'settings';
  page.hidden = true;
  page.innerHTML = `
    <div class="settings-card" role="dialog" aria-modal="true" aria-label="Settings">
      <header><h2>Settings</h2><button id="settings-close" aria-label="Close settings" title="Close (Esc)">×</button></header>
      <div class="settings-body"></div>
    </div>`;
  page.querySelector('.settings-body')!.append(...(extra.sections ?? []), lookSection());
  const close = () => {
    page.hidden = true;
  };
  const open = () => {
    page.hidden = false;
    extra.onOpen?.();
    page.querySelector<HTMLElement>('#settings-close')!.focus();
  };
  page.addEventListener('mousedown', event => {
    if (event.target === page) close();
  });
  page.querySelector('#settings-close')!.addEventListener('click', close);
  return {
    element: page,
    close,
    toggle: () => (page.hidden ? open() : close()),
    get isOpen() {
      return !page.hidden;
    },
  };
}
