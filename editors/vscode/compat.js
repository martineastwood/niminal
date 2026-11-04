/** What to tell the player when the Stage they connected to and this extension are not from the same release, or null.
 *  A development build of either has no release, so there is nothing to compare. */
function releaseMismatch(stageRelease, extensionVersion) {
  if (!stageRelease || !extensionVersion || stageRelease === extensionVersion) return null;
  return `Niminal Stage is release ${stageRelease} but the VS Code extension is ${extensionVersion}. Update both from the same download, or things may not work.`;
}

module.exports = { releaseMismatch };
