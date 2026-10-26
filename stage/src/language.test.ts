import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { EditorState } from '@codemirror/state';
import { ensureSyntaxTree } from '@codemirror/language';
import { commands, constants, keywords, niminal, types } from '../../editors/codemirror/language';

/** (token name, text) for each coloured stretch of `doc`. */
function tokens(doc: string) {
  const state = EditorState.create({ doc, extensions: niminal });
  const tree = ensureSyntaxTree(state, doc.length, 1000)!;
  const out: [string, string][] = [];
  tree.iterate({
    enter: node => {
      if (node.from < node.to && node.name !== 'Document') out.push([node.name, doc.slice(node.from, node.to)]);
    },
  });
  return out;
}

describe('the niminal language colours', () => {
  it('colours clips, kits, grids and the commands that play them', () => {
    const found = tokens(
      'drums = kit {\n  kick: bd\n}\nclip beat {\n  notes: grid {\n    kick:  x..x ..X.\n    hat:   [x*2 ~]\n  }\n}\nbeat.start\nbeat.reverse',
    );
    const has = (name: string, text: string) => expect(found).toContainEqual([name, text]);
    has('keyword', 'kit');
    has('keyword', 'clip');
    has('typeName', 'beat');
    has('keyword', 'grid');
    has('typeName', 'kick');
    has('string', 'x');
    has('string', 'X');
    has('comment', '..');
    has('typeName', 'hat');
    has('keyword', 'start');
    has('keyword', 'reverse');
  });

  it('stops colouring as a grid when the grid ends', () => {
    const found = tokens('grid { kick: x }\nx = 1');
    const xs = found.filter(([, text]) => text === 'x').map(([name]) => name);
    expect(xs).toEqual(['string', 'variableName']);
  });
});

/** The words a TextMate rule of this name matches, from its `(a|b|c)` alternation. */
function grammarWords(name: string) {
  const grammar = readFileSync('../editors/vscode/syntaxes/niminal.tmLanguage.json', 'utf8');
  const found: string[] = [];
  JSON.parse(grammar, (_, value) => {
    if (value && typeof value === 'object' && value.name === name && typeof value.match === 'string') {
      const alternation = value.match.match(/\(([\w%?]+(?:\|[\w%?]+)+)\)/)![1].split('|');
      // `beats?` matches `beat` and `beats`
      for (const word of alternation) found.push(...(word.endsWith('?') ? [word.slice(0, -2), word.slice(0, -1)] : [word]));
    }
    return value;
  });
  return new Set(found);
}

describe('the editors agree with the VS Code grammar', () => {
  // `grid` has a rule of its own in the grammar, and `out` is coloured as a name there
  const own = new Set(['grid', 'out']);

  it('on keywords', () => {
    const shared = [...keywords].filter(word => !own.has(word)).sort();
    expect(shared).toEqual([...grammarWords('keyword.control.niminal')].sort());
  });

  it('on commands', () => {
    expect([...commands].sort()).toEqual([...grammarWords('keyword.control.command.niminal')].sort());
  });

  it('on constants', () => {
    // the grammar lists `mono` with the constants and the keywords alike
    const shared = [...constants].sort();
    expect(shared).toEqual([...grammarWords('constant.language.wave.niminal')].filter(w => w !== 'mono').sort());
  });

  it('on units', () => {
    const grammar = [...grammarWords('constant.numeric.unit.niminal')];
    const units = types;
    for (const unit of grammar) {
      if (unit === 's' || unit === '%') continue;
      expect(units.has(unit), `unit ${unit}`).toBe(true);
    }
  });
});
