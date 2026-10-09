// What the performance panel shows for a track or scene.

function trackDescription(track) {
  const parts = [track.clip ? `▶ ${track.clip}` : "stopped"];
  if (track.muted) parts.push("muted");
  if (track.soloed) parts.push("solo");
  return parts.join(" · ");
}

/** The commands a panel button sends, as niminal code. */
const commands = {
  launch: (scene) => `launch ${scene}`,
  mute: (track) => (track.muted ? `unmute ${track.name}` : `mute ${track.name}`),
  solo: (track) => (track.soloed ? `unsolo ${track.name}` : `solo ${track.name}`),
  stop: (track) => `stop ${track.name}`,
};

module.exports = { trackDescription, commands };
