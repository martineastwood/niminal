//! What each track is playing, kept as text so that redefining a pattern can start the new version.

use super::*;

/// What a track is playing: a clip, as written, and the changes made to it live with
/// `bass.reverse` and the like, in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Playing {
    pub(super) base: String,
    pub(super) edits: Vec<String>,
}

/// The most changes one track can carry.
pub(super) const MAX_EDITS: usize = 16;

impl Playing {
    pub(super) fn new(base: impl Into<String>) -> Playing {
        Playing { base: base.into(), edits: Vec::new() }
    }

    /// `riff.reverse.thin(25%)`
    pub(super) fn text(&self) -> String {
        self.edits.iter().fold(self.base.clone(), |text, edit| format!("{text}.{edit}"))
    }
}

/// `bass.reverse` becomes `bass.play(<what bass is playing>.reverse)`, `bass.undo` takes
/// the last change off, `bass.undo(reverse)` the last `reverse`, and `bass.clear` all of
/// them. Returns the statements to compile and what each track is playing after them.
///
/// The name before the dot is usually a part. For a clip that is only material, such as
/// `answer`, the change goes to every part playing it.
pub(super) fn join_edit(statement: &Statement, playing: &BTreeMap<String, Playing>, project: &Project) -> Result<Vec<Edited>, Vec<Problem>> {
    let (Some(range), Some(plays)) = (&statement.edit, &statement.plays) else { return Ok(vec![(statement.clone(), None)]) };
    let name = plays.track.clone();
    if plays.what.trim() == "start" {
        // `verse.start` launches the scene of that name, `bass.start` plays the clip of that name
        let mut statement = statement.clone();
        statement.edit = None;
        if project.text_of(&format!("scene:{name}")).is_some() {
            statement.plays = None;
            statement.launch = Some(name);
            statement.write_key = Some("launch".into());
        } else {
            statement.write_key = Some(format!("play:{name}"));
            statement.plays = Some(niminal_lang::PlayInfo { track: name.clone(), what: name });
        }
        return Ok(vec![(statement, None)]);
    }
    let parts: Vec<&String> = if playing.contains_key(&name) {
        vec![&name]
    } else {
        playing.iter().filter(|(_, p)| root_name(&p.base) == name).map(|(track, _)| track).collect()
    };
    if parts.is_empty() {
        return Err(vec![Problem::plain(format!(
            "`{name}` isn't playing anything to change; start it first, for example `{name}.start`, or start a scene that has it"
        ))]);
    }
    let mut out = Vec::new();
    for track in parts {
        let next = change_playing(&playing[track], plays.what.trim(), track)?;
        let text = next.text();
        let mut one = statement.clone();
        one.text.replace_range(range.clone(), &format!("{track}.play({text})"));
        one.plays = Some(niminal_lang::PlayInfo { track: track.clone(), what: text });
        one.write_key = Some(format!("play:{track}"));
        one.edit = None;
        out.push((one, Some((track.clone(), next))));
    }
    Ok(out)
}

/// What a track plays after `change`: one more change, or `undo`, `undo(name)` or `clear`.
pub(super) fn change_playing(current: &Playing, change: &str, track: &str) -> Result<Playing, Vec<Problem>> {
    let mut next = current.clone();
    let problem = |message: String| Err(vec![Problem::plain(message)]);
    match change {
        "undo" => {
            if next.edits.pop().is_none() {
                return problem(format!("nothing to undo: `{track}` has no changes made to it"));
            }
        }
        "clear" => next.edits.clear(),
        named if named.starts_with("undo(") && named.ends_with(')') => {
            let wanted = named[5..named.len() - 1].trim();
            match next.edits.iter().rposition(|edit| root_name(edit) == wanted) {
                Some(at) => {
                    next.edits.remove(at);
                }
                None => return problem(format!("`{track}` has no `{wanted}` change to undo")),
            }
        }
        change if next.edits.len() >= MAX_EDITS => {
            return problem(format!(
                "`{track}` already has {MAX_EDITS} changes (`{change}` would be one more); `{track}.undo` or `{track}.clear` takes some off"
            ));
        }
        change => next.edits.push(change.to_string()),
    }
    Ok(next)
}

/// The first name in an expression: `riff` in `riff.reverse.thin(25%)`, `thin` in `thin(25%)`.
pub(super) fn root_name(text: &str) -> &str {
    let text = text.trim_start();
    let end = text.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(text.len());
    &text[..end]
}

/// Record what a launched scene plays: its tracks play what it says, and every other
/// track with an instrument stops.
pub(super) fn apply_launch(playing: &mut BTreeMap<String, Playing>, program: &Program, project: &Project, scene: &str) {
    let Some(text) = project.text_of(&format!("scene:{scene}")) else { return };
    let entries = niminal_lang::scene_entries(text);
    for track in program.tracks.iter().filter(|t| t.instrument.is_some()) {
        if !entries.iter().any(|(name, _)| *name == track.name) {
            playing.remove(&track.name);
        }
    }
    for (track, what) in entries {
        match what {
            Some(what) => playing.insert(track, Playing::new(what)),
            None => playing.remove(&track),
        };
    }
}

/// Update the record of what each track is playing for a statement that has
/// been compiled: `play` records its source, `stop` and `hush` forget.
pub(super) fn apply_to_playing(playing: &mut BTreeMap<String, Playing>, program: &Program, statement: &Statement) {
    for scheduled in &program.performance {
        match name_action(program, &scheduled.action) {
            NamedAction::Play { track, .. } => {
                if let Some(plays) = statement.plays.as_ref().filter(|p| p.track == track) {
                    playing.insert(track, Playing::new(plays.what.clone()));
                }
            }
            NamedAction::Stop(track) => {
                playing.remove(&track);
            }
            NamedAction::Hush | NamedAction::Panic | NamedAction::Reset => playing.clear(),
            _ => {}
        }
    }
}

/// Does `source` use any of `names` as a name?
pub(super) fn mentions_any(source: &str, names: &[String]) -> bool {
    source
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|word| names.iter().any(|n| n == word))
}
