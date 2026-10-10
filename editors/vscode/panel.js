// What the performance panel shows for a track or scene.

function trackDescription(track) {
  const parts = [track.clip ? `▶ ${track.clip}` : "stopped"];
  if (track.edits && track.edits.length) parts.push(track.edits.join(" → "));
  if (track.muted) parts.push("muted");
  if (track.soloed) parts.push("solo");
  return parts.join(" · ");
}

/** The commands a panel button sends, as niminal code. */
const commands = {
  launch: (scene) => `${scene}.start`,
  mute: (track) => (track.muted ? `${track.name}.unmute` : `${track.name}.mute`),
  solo: (track) => (track.soloed ? `${track.name}.unsolo` : `${track.name}.solo`),
  stop: (track) => `${track.name}.stop`,
};

module.exports = { trackDescription, commands };
