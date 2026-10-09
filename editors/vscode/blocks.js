// Finding the code to evaluate: the lines around the cursor up to the
// nearest blank lines, widened so a block with blank lines inside its
// brackets is not cut in half.

function bracketDelta(line) {
  const code = line.split("//")[0];
  let depth = 0;
  for (const c of code) {
    if ("{[(".includes(c)) depth++;
    else if ("}])".includes(c)) depth--;
  }
  return depth;
}

/** The first and last line (0-based, inclusive) of the block holding `line`, or null on a blank line. */
function blockAround(lines, line) {
  if (!lines[line] || lines[line].trim() === "") return null;
  let start = line;
  while (start > 0 && lines[start - 1].trim() !== "") start--;
  let end = line;
  while (end + 1 < lines.length && lines[end + 1].trim() !== "") end++;

  // Blank lines inside brackets don't end a block: reach past them, upward
  // while a bracket is closed that was never opened, downward while one is left open.
  for (;;) {
    const depth = lines.slice(start, end + 1).reduce((d, l) => d + bracketDelta(l), 0);
    if (depth < 0 && start > 0) {
      start--;
      while (start > 0 && (lines[start].trim() === "" || lines[start - 1].trim() !== "")) start--;
    } else if (depth > 0 && end + 1 < lines.length) {
      end++;
      while (end + 1 < lines.length && (lines[end].trim() === "" || lines[end + 1].trim() !== "")) end++;
    } else {
      break;
    }
  }
  return { start, end };
}

module.exports = { blockAround };
