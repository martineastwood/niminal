// `ctl cutoff = midi.learn.range(200hz..4khz)` is rewritten to the control change the server
// heard first: `ctl cutoff = midi.cc(21).range(200hz..4khz)`.

/** The text with `midi.learn` of the control called `control` replaced by `source`, or null if it has none. */
function rewriteLearned(text, control, source) {
  const pattern = new RegExp(`(\\bctl\\s+${control.replace(/[^\w]/g, "\\$&")}\\s*=\\s*)midi\\.learn\\b`);
  return pattern.test(text) ? text.replace(pattern, `$1${source}`) : null;
}

module.exports = { rewriteLearned };
