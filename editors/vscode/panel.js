// What the performance panel shows for a part.

function partDescription(part) {
  const parts = [part.clip ? `▶ ${part.clip}` : "stopped"];
  if (part.edits && part.edits.length) parts.push(part.edits.join(" → "));
  if (part.muted) parts.push("muted");
  if (part.soloed) parts.push("solo");
  return parts.join(" · ");
}

module.exports = { partDescription };
