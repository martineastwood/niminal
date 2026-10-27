import type { ShortcutSection } from './palette';

/** The keys, as shown by Cmd/Ctrl+/ and F1. The grid's are those of its keydown handler; see the editing reference. */
export const shortcutSections: ShortcutSection[] = [
  { title: 'Anywhere', rows: [
    ['Cmd/Ctrl+K', 'Command palette: every action, searchable'],
    ['Cmd/Ctrl+/ · F1', 'This list'],
    ['Ctrl+1 … 5', 'Go to the grid, sequence, instruments, sound browser, code'],
    ['Cmd/Ctrl+\\', 'Show or hide the sequence bar'],
    ['Cmd/Ctrl+Shift+B', 'Show or hide the right bar'],
    ['Cmd/Ctrl+Shift+F', 'Focus mode: only the grid'],
    ['Cmd/Ctrl+B', 'Sound browser'],
    ['Cmd/Ctrl+= / − / 0', 'Interface larger, smaller, actual size (the code font while in the editor)'],
    ['F5 · F8', 'Play the song · stop'],
    ['Esc', 'Record mode on or off'],
    ['Cmd/Ctrl+Z · Shift+Z', 'Undo · redo'],
    ['F6', 'Move between the grid and the code editor'],
    ['Option+↑ / ↓', 'Previous or next instrument'],
  ] },
  { title: 'Grid', rows: [
    ['Space', 'Play or stop the pattern'],
    ['Arrows', 'Move the cursor'],
    ['Shift+arrows', 'Select a block'],
    ['Tab · Shift+Tab', 'Next or previous track'],
    ['PageUp / PageDown', 'A bar up or down'],
    ['Home · End', 'First or last row'],
    ['F9 · F10 · F11 · F12', 'Start, quarter, half, three quarters of the pattern'],
    ['Cmd/Ctrl+A', 'Select all'],
    ['Cmd/Ctrl+Shift+A', 'Select the whole track'],
  ] },
  { title: 'Recording (Esc to turn it on)', rows: [
    ['Z S X D C V … · Q 2 W 3 E R …', 'Piano keys: the current octave, and the one above'],
    ['A', 'Note off (release)'],
    ['−', 'Cut the note at once'],
    ['Letters and digits on an Fx cell', 'A command: S80 start, B00 backwards, G10 glide, U08 / D08 slide, R03 retrigger, C03 cut, Y80 chance, I20 / O20 fade; L80 track volume, P80 pan, 12FF an effect\'s parameter'],
    ['Delete', 'Clear the selection'],
    ['Insert · Backspace', 'Insert or remove a row'],
    ['Cmd/Ctrl+I', 'Interpolate between the selected rows'],
    ['Cmd/Ctrl+D', 'Copy the selection below it'],
    ['Cmd/Ctrl+Option+↑ / ↓', 'Transpose by a semitone'],
    ['Cmd/Ctrl+Shift+↑ / ↓', 'Transpose by an octave'],
  ] },
  { title: 'Sequence', rows: [
    ['Cmd/Ctrl+↑ / ↓', 'Previous or next sequence step'],
    ['Cmd/Ctrl+Home / End', 'First or last step'],
    ['Cmd/Ctrl+← / →', 'Previous or next pattern in this step'],
    ['Cmd/Ctrl+Insert · Delete', 'New step · remove the step'],
  ] },
  { title: 'Code editor', rows: [
    ['Cmd/Ctrl+Enter', 'Apply and audition'],
    ['Cmd/Ctrl+= / −', 'Larger or smaller code font'],
    ['F6', 'Back to the grid'],
  ] },
];
