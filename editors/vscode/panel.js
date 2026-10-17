// What the performance panel shows for a track.

function trackDescription(track) {
  const parts = [track.clip ? `▶ ${track.clip}` : "stopped"];
  if (track.edits && track.edits.length) parts.push(track.edits.join(" → "));
  if (track.muted) parts.push("muted");
  if (track.soloed) parts.push("solo");
  return parts.join(" · ");
}

module.exports = { trackDescription };
