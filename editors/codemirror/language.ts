import { HighlightStyle, StreamLanguage, StringStream, syntaxHighlighting } from '@codemirror/language';
import { tags } from '@lezer/highlight';

// The one lexer and palette the stage and the tracker share, so code reads the same in both.
// The VS Code grammar (editors/vscode/syntaxes) lists the same words; language.test.ts keeps them
// from drifting apart.

export const keywords = new Set('instr mono fn def const bus buffer ctl node fx config clip scene arrangement kit grid events midi_out over meter tempo state use unsolo hush panic reset reset_on after new at next now if else match for in and or not init control as capture last spawn oversample env cycle out'.split(' '));
export const types = new Set('hz khz db sec ms samples beat beats bar bars deg st ct bpm int num'.split(' '));
/** Words after a dot that act on a clip or scene: `beat.start`, `bass.reverse`. */
export const commands = new Set('start stop play mute unmute solo unsolo undo clear set reverse thin'.split(' '));
/** Words that name what they define: `clip beat`, `scene verse`. */
const definers = new Set('clip scene arrangement'.split(' '));
export const constants = new Set('sine saw square pulse tri stereo quad true false pi sample_rate'.split(' '));

// Lexical colouring remains available while a definition is incomplete. Semantic
// information, documentation and validation come from the compiler through LSP.
export const niminal = StreamLanguage.define({
  name: 'niminal',
  startState: () => ({ string: false, grid: null as null | { close: string; depth: number }, naming: false, dot: false }),
  token(stream, state) {
    if (state.string) {
      while (!stream.eol()) {
        const ch = stream.next();
        if (ch === '\\') stream.next();
        else if (ch === '"') { state.string = false; break; }
      }
      return 'string';
    }
    if (stream.eatSpace()) return null;
    if (state.grid) return gridToken(stream, state);
    if (stream.match('//')) { stream.skipToEnd(); return 'comment'; }
    if (stream.peek() === '"') { stream.next(); state.string = true; return 'string'; }
    if (stream.match(/\d+(?:\.\d+)?(?:khz|hz|db|bpm|sec|ms|samples|beats?|bars?|deg|st|ct|s|%)?(?![\w])/)) return 'number';
    if (stream.match(/[a-g](?:#|b)?-?\d\b/)) return 'atom';
    if (stream.match(/[A-Za-z_][\w]*/)) {
      const word = stream.current();
      const named = state.naming;
      const after = state.dot;
      state.naming = definers.has(word);
      state.dot = false;
      if (word === 'grid' && stream.match(/^\s*[{[]/, false)) {
        state.grid = { close: stream.match(/^\s*\{/, false) ? '}' : ']', depth: 0 };
        return 'keyword';
      }
      if (named) return 'typeName';
      if (after && commands.has(word)) return 'keyword';
      if (keywords.has(word)) return 'keyword';
      if (types.has(word)) return 'typeName';
      if (constants.has(word)) return 'atom';
      if (stream.match(/^\s*\(/, false)) return 'variableName.function';
      return 'variableName';
    }
    if (stream.match(/[-+*/=<>!|~]+/)) return 'operator';
    state.dot = stream.next() === '.';
    return 'punctuation';
  },
  languageData: { commentTokens: { line: '//' }, closeBrackets: { brackets: ['(', '[', '{', '"'] } },
});

type Grid = { close: string; depth: number };

/** Inside `grid { ... }` or `grid[ ... ]`: row names, hits (`x X o`) and rests (`. ~`). */
function gridToken(stream: StringStream, state: { grid: Grid | null }) {
  const grid = state.grid!;
  if (stream.match('//')) {
    stream.skipToEnd();
    return 'comment';
  }
  const open = grid.close === '}' ? '{' : '[';
  const ch = stream.peek();
  if (ch === open) {
    stream.next();
    grid.depth++;
    return 'punctuation';
  }
  if (ch === grid.close) {
    stream.next();
    if (--grid.depth <= 0) state.grid = null;
    return 'punctuation';
  }
  if (stream.match(/[xXo]+(?![\w])/)) return 'string';
  if (stream.match(/[.~]+/)) return 'comment';
  if (stream.match(/\d+(?:\.\d+)?(?:khz|hz|db|bpm|sec|ms|samples|beats?|bars?|deg|st|ct|s|%)?(?![\w])/)) return 'number';
  if (stream.match(/[A-Za-z_][\w.]*/)) return stream.sol() || stream.string.slice(0, stream.start).trim() === '' ? 'typeName' : 'variableName';
  stream.next();
  return 'punctuation';
}

export const colours = syntaxHighlighting(
  HighlightStyle.define([
    { tag: tags.keyword, color: '#c6a1e8' },
    { tag: tags.comment, color: '#819184' },
    { tag: tags.string, color: '#a5c98d' },
    { tag: [tags.number, tags.atom], color: '#e9b66e' },
    { tag: tags.typeName, color: '#74c7c3' },
    { tag: tags.function(tags.variableName), color: '#8cbce7' },
  ]),
);
