// What the performance panel and the status bar show.

/** What the clips panel shows for a clip: whether it plays, what has been done to it, and its mute and solo. */
function clipDescription(clip) {
  const parts = [clip.running ? "▶" : "stopped"];
  if (clip.edits && clip.edits.length) parts.push(clip.edits.join(" → "));
  if (clip.muted) parts.push("muted");
  if (clip.soloed) parts.push("solo");
  return parts.join(" · ");
}

/** What the scenes panel shows for a scene: its shortcut, and how many of its clips are playing. */
function sceneDescription(scene, shortcut) {
  const state = scene.active ? "▶ all playing" : scene.running > 0 ? `${scene.running}/${scene.clips.length} playing` : "";
  return [shortcut, state].filter(Boolean).join(" · ");
}

/** The state that can silence a performance without being obvious: a soloed clip hides every other, a muted one is gone. */
function silencers(clips) {
  const soloed = clips.filter((c) => c.soloed).map((c) => c.name);
  const muted = clips.filter((c) => c.muted).length;
  const parts = [];
  if (soloed.length) parts.push(`SOLO ${soloed.join(", ")}`);
  if (muted) parts.push(`${muted} muted`);
  return parts.join(" · ");
}

module.exports = { clipDescription, sceneDescription, silencers };
