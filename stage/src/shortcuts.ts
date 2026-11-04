/** The key that the shortcuts are written with: Cmd on a Mac, Ctrl elsewhere. */
export const mod = typeof navigator !== 'undefined' && /Mac/i.test(navigator.platform) ? '⌘' : 'Ctrl+';

/** The shortcuts the stage teaches, as shown on its ready screen. */
export const shortcuts: [keys: string, what: string][] = [
  [`${mod}↩`, 'evaluate the selection, or the block the cursor is in'],
  [`⇧${mod}↩`, 'evaluate the whole file'],
  [`${mod}1…9`, 'start or stop a scene'],
  ['⌥ drag a number', 'drag a number, and send it when you let go (⇧ for tens, Esc to cancel)'],
  [`${mod}⌥↑ ↓`, 'nudge the number at the cursor; send it yourself'],
  [`${mod}.`, 'hush: stop every clip'],
  [`⇧${mod}.`, 'panic: silence everything now, tails included'],
  [`⇧${mod}L`, 'browse files: open pieces, hear samples, write them into the code'],
  [`${mod}B`, 'show or hide the sidebar'],
  [`⇧${mod}P`, 'performance mode'],
  [`${mod}+ ${mod}−`, 'text size (or pinch)'],
];
