import { StreamLanguage } from '@codemirror/language';

const keywords = new Set('instr mono fn def const bus buffer ctl node fx config state use reset reset_on after new at next now if else match for in and or not init control as capture last spawn oversample env cycle out'.split(' '));
const types = new Set('hz khz db sec ms samples beat beats bar bars deg st ct bpm int num'.split(' '));
const constants = new Set('sine saw square pulse tri stereo quad true false pi sample_rate'.split(' '));

// Lexical colouring remains available while a definition is incomplete. Semantic
// information, documentation and validation come from the compiler through LSP.
export const niminal = StreamLanguage.define({
  name: 'niminal',
  startState: () => ({ string: false }),
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
    if (stream.match('//')) { stream.skipToEnd(); return 'comment'; }
    if (stream.peek() === '"') { stream.next(); state.string = true; return 'string'; }
    if (stream.match(/\d+(?:\.\d+)?(?:khz|hz|db|bpm|sec|ms|samples|beats?|bars?|deg|st|ct|s|%)?(?![\w])/)) return 'number';
    if (stream.match(/[a-g](?:#|b)?-?\d\b/)) return 'atom';
    if (stream.match(/[A-Za-z_][\w]*/)) {
      const word = stream.current();
      if (keywords.has(word)) return 'keyword';
      if (types.has(word)) return 'typeName';
      if (constants.has(word)) return 'atom';
      if (stream.match(/^\s*\(/, false)) return 'variableName.function';
      return 'variableName';
    }
    if (stream.match(/[-+*/=<>!|~]+/)) return 'operator';
    stream.next();
    return 'punctuation';
  },
  languageData: { commentTokens: { line: '//' }, closeBrackets: { brackets: ['(', '[', '{', '"'] } },
});
