// On a touch screen the keys code needs most are two taps away. This row sits above the on-screen keyboard
// while the editor has focus: brackets that come in pairs, the marks the language uses, Tab, arrows and Run.
import { cursorCharLeft, cursorCharRight, indentMore } from '@codemirror/commands';
import type { CodeEditor } from './editor';

type Key = { label: string; aria?: string; run: (editor: CodeEditor) => void };

const insert = (text: string, inside = 0) => (editor: CodeEditor) => {
  const view = editor.view;
  const { from, to } = view.state.selection.main;
  view.dispatch({ changes: { from, to, insert: text }, selection: { anchor: from + text.length - inside }, scrollIntoView: true });
};

export function setupKeyRow(row: HTMLElement, editor: CodeEditor, onRun: () => void) {
  const keys: Key[] = [
    { label: '▶', aria: 'Run this block', run: () => onRun() },
    { label: '{ }', run: insert('{}', 1) },
    { label: '[ ]', run: insert('[]', 1) },
    { label: '( )', run: insert('()', 1) },
    { label: '~', run: insert('~') },
    { label: '.', run: insert('.') },
    { label: ':', run: insert(':') },
    { label: '=', run: insert('=') },
    { label: ',', run: insert(',') },
    { label: '"', run: insert('""', 1) },
    { label: 'Tab', run: (e) => indentMore(e.view) },
    { label: '←', aria: 'Cursor left', run: (e) => cursorCharLeft(e.view) },
    { label: '→', aria: 'Cursor right', run: (e) => cursorCharRight(e.view) },
  ];
  for (const key of keys) {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = key.label;
    if (key.aria) button.setAttribute('aria-label', key.aria);
    // pressing must not take focus from the editor, or the keyboard would close
    button.addEventListener('pointerdown', (event) => { event.preventDefault(); key.run(editor); });
    row.append(button);
  }

  const touch = window.matchMedia('(pointer: coarse)');
  // the page leaves room for the row (it overlaps the bottom of the page), and closes up again when it goes
  const show = () => { if (touch.matches) { row.dataset.open = ''; document.body.dataset.keys = ''; } };
  const hide = () => { delete row.dataset.open; delete document.body.dataset.keys; };
  editor.view.dom.addEventListener('focusin', show);
  editor.view.dom.addEventListener('focusout', hide);

  // the on-screen keyboard covers the bottom of the page without resizing it: follow what is actually visible
  const viewport = window.visualViewport;
  if (viewport) {
    const place = () => {
      const covered = window.innerHeight - viewport.height - viewport.offsetTop;
      row.style.transform = `translateY(${-Math.max(0, covered)}px)`;
    };
    viewport.addEventListener('resize', place);
    viewport.addEventListener('scroll', place);
    place();
  }
}
