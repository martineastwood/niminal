/** The key that the shortcuts are written with: Cmd on a Mac, Ctrl elsewhere. */
export const mod = typeof navigator !== 'undefined' && /Mac/i.test(navigator.platform) ? '⌘' : 'Ctrl+';

/** The shortcuts the stage teaches, as shown on its ready screen. */
export const shortcuts: [keys: string, what: string][] = [
  [`${mod}↩`, 'evaluate the selection, or the statement the cursor is in'],
  ['⇧↩', 'evaluate like ' + mod + '↩, then move to the next statement'],
  ['⌥↩', 'evaluate the lines around the cursor, up to the blank lines'],
  [`⇧${mod}↩`, 'evaluate the whole file'],
  [`${mod}1…9`, 'start or stop a scene'],
  ['⌥ drag a number', 'drag a number, and send it when you let go (⇧ for tens, Esc to cancel)'],
  [`${mod}⌥↑ ↓`, 'nudge the number at the cursor; send it yourself'],
  [`${mod}.`, 'hush: stop every clip'],
  [`⇧${mod}.`, 'panic: silence everything now, tails included'],
  ['drop audio on the code', 'write sample("…") where it lands; drop a .nml to open it'],
  [`${mod}B`, 'show or hide the sidebar'],
  [`${mod}+ ${mod}−`, 'text size (or pinch)'],
];
