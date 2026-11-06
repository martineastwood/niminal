// Finding the code to evaluate: the statement the cursor is in. A statement starts at a line outside
// any brackets and runs through every line that belongs to it: lines inside its brackets (blank ones
// too), `.method` lines, and lines indented further than its first line. So Cmd+Enter on a line inside
// an `instr { ... }` evaluates the whole instrument, and on `melody.start` just that line, however
// close its neighbours are. `paragraphAround` is the wider choice: the lines up to the nearest blank lines. The VS Code extension does the same in editors/vscode/blocks.js, so
// Cmd+Enter picks the same code in both.

/** The code of a line without its `//` comment and the contents of its strings. */
function codeOf(line: string) {
  let code = '';
  let inString = false;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (c === '"') inString = !inString;
    else if (!inString) {
      if (c === '/' && line[i + 1] === '/') break;
      code += c;
    }
  }
  return code;
}

/** Every statement of the file as first and last line (0-based, inclusive), in order. */
function statements(lines: string[]) {
  let depth = 0;
  const found: { start: number; end: number; indent: number }[] = [];
  for (let i = 0; i < lines.length; i++) {
    const code = codeOf(lines[i]);
    const text = code.trim();
    if (text === '') continue;
    const indent = code.length - code.trimStart().length;
    const last = found[found.length - 1];
    if (last && (depth > 0 || text.startsWith('.') || indent > last.indent)) last.end = i;
    else found.push({ start: i, end: i, indent });
    for (const c of code) {
      if ('{[('.includes(c)) depth++;
      else if ('}])'.includes(c)) depth = Math.max(0, depth - 1);
    }
  }
  return found;
}

/** The first and last line (0-based, inclusive) of the statement holding `line`, or null between statements. */
export function blockAround(lines: string[], line: number) {
  const hit = statements(lines).find(s => s.start <= line && line <= s.end);
  return hit ? { start: hit.start, end: hit.end } : null;
}

/** The first line of the first statement that starts after `line`, or null at the end of the file. */
export function nextStatement(lines: string[], line: number) {
  return statements(lines).find(s => s.start > line)?.start ?? null;
}

function bracketDelta(line: string) {
  let depth = 0;
  for (const c of codeOf(line)) {
    if ('{[('.includes(c)) depth++;
    else if ('}])'.includes(c)) depth--;
  }
  return depth;
}

/** The first and last line (0-based, inclusive) of the lines around `line` up to the nearest blank lines, widened so brackets balance; null on a blank line. */
export function paragraphAround(lines: string[], line: number) {
  if (!lines[line] || lines[line].trim() === '') return null;
  let start = line;
  while (start > 0 && lines[start - 1].trim() !== '') start--;
  let end = line;
  while (end + 1 < lines.length && lines[end + 1].trim() !== '') end++;
  for (;;) {
    const depth = lines.slice(start, end + 1).reduce((d, l) => d + bracketDelta(l), 0);
    if (depth < 0 && start > 0) {
      start--;
      while (start > 0 && (lines[start].trim() === '' || lines[start - 1].trim() !== '')) start--;
    } else if (depth > 0 && end + 1 < lines.length) {
      end++;
      while (end + 1 < lines.length && (lines[end].trim() === '' || lines[end + 1].trim() !== '')) end++;
    } else {
      break;
    }
  }
  return { start, end };
}
