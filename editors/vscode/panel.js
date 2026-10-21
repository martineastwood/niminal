// What the performance panel shows for a clip.

function clipDescription(clip) {
  const parts = [clip.clip ? `▶ ${clip.clip}` : "stopped"];
  if (clip.edits && clip.edits.length) parts.push(clip.edits.join(" → "));
  if (clip.muted) parts.push("muted");
  if (clip.soloed) parts.push("solo");
  return parts.join(" · ");
}

module.exports = { clipDescription };
