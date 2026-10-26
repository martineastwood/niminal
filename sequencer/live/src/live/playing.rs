//! What each running clip has had done to it, kept as text so that redefining a pattern can apply the new version.

use super::*;

/// A running clip: the changes made to it live with `bass.reverse` and the like, in order.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Playing {
    pub(super) edits: Vec<String>,
    /// The sample it is stopped at, if it is. Kept until that moment is past, because a start for the same
    /// moment undoes the stop, and the clip carries on with what had been done to it.
    pub(super) stopped_at: Option<u64>,
}

/// The most changes one clip can carry.
pub(super) const MAX_EDITS: usize = 16;

impl Playing {
    pub(super) fn running(&self) -> bool {
        self.stopped_at.is_none()
    }

    /// `riff.reverse.drop(25%)`
    pub(super) fn text(&self, clip: &str) -> String {
        self.edits
            .iter()
            .fold(clip.to_string(), |text, edit| format!("{text}.{edit}"))
    }
}

/// `bass.reverse` becomes `bass.__update(bass.reverse)`, which changes the clip that is running and keeps its
/// place in the loop. `bass.undo` takes the last change off, `bass.undo(reverse)` the last `reverse`, and
/// `bass.clear` all of them. Returns the statements to compile and what each clip carries after them.
pub(super) fn join_edit(
    statement: &Statement,
    playing: &BTreeMap<String, Playing>,
    project: &Project,
) -> Result<Vec<Edited>, Vec<Problem>> {
    let StatementData::Command {
        action: CommandAction::Edit { plays, range },
        ..
    } = &statement.data
    else {
        return Ok(vec![(statement.clone(), None)]);
    };
    let (name, change, range) = (plays.node.clone(), plays.what.clone(), range.clone());
    if !playing.get(&name).is_some_and(Playing::running) {
        let scene = project.text_of(&format!("scene:{name}"));
        let message = if let Some(text) = scene {
            let clip = niminal_compose::scene_clips(text)
                .into_iter()
                .next()
                .unwrap_or_else(|| "bass".into());
            format!(
                "`{name}` is a scene, which starts and stops clips; change one of its clips, for example `{clip}.{}`",
                change.trim()
            )
        } else {
            format!(
                "`{name}` isn't playing anything to change; start it first, for example `{name}.start`, or start a scene that has it"
            )
        };
        return Err(vec![Problem::plain(message)]);
    }
    let next = change_playing(&playing[&name], change.trim(), &name)?;
    let text = next.text(&name);
    let mut one = statement.clone();
    one.text
        .replace_range(range, &format!("{name}.__update({text})"));
    one.write_key = Some(format!("play:{name}"));
    one.set_action(CommandAction::Play(PlayInfo {
        node: name.clone(),
        what: text,
    }));
    Ok(vec![(one, Some((name, next)))])
}

/// What a clip carries after `change`: one more change, or `undo`, `undo(name)` or `clear`.
pub(super) fn change_playing(
    current: &Playing,
    change: &str,
    node: &str,
) -> Result<Playing, Vec<Problem>> {
    let mut next = current.clone();
    let problem = |message: String| Err(vec![Problem::plain(message)]);
    match change {
        "undo" => {
            if next.edits.pop().is_none() {
                return problem(format!(
                    "nothing to undo: `{node}` has no changes made to it"
                ));
            }
        }
        "clear" => next.edits.clear(),
        named if named.starts_with("undo(") && named.ends_with(')') => {
            let wanted = named[5..named.len() - 1].trim();
            match next
                .edits
                .iter()
                .rposition(|edit| root_name(edit) == wanted)
            {
                Some(at) => {
                    next.edits.remove(at);
                }
                None => return problem(format!("`{node}` has no `{wanted}` change to undo")),
            }
        }
        change if next.edits.len() >= MAX_EDITS => {
            return problem(format!(
                "`{node}` already has {MAX_EDITS} changes (`{change}` would be one more); `{node}.undo` or `{node}.clear` takes some off"
            ));
        }
        change => next.edits.push(change.to_string()),
    }
    Ok(next)
}

/// The first name in an expression: `riff` in `riff.reverse.drop(25%)`, `drop` in `drop(25%)`.
pub(super) fn root_name(text: &str) -> &str {
    let text = text.trim_start();
    let end = text
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    &text[..end]
}

/// Update the record of which clips are running for a statement that has been compiled, which lands at
/// `lands_at`: a start records the clip, a stop or `hush` forgets it. A clip that is stopped and started again
/// for the same moment carries on, as it does in the schedule, so it keeps what it had been changed to.
pub(super) fn apply_to_playing(
    playing: &mut BTreeMap<String, Playing>,
    lands_at: u64,
    program: &Composition,
) {
    for scheduled in &program.performance {
        // a line for later, such as `at +4bars chorus.start`, is not what is playing now
        if scheduled.at.to_seconds(program.tempo) > 0.0 {
            continue;
        }
        match name_action(program, &scheduled.action) {
            Some(NamedAction::Start { node, .. }) => match playing.get_mut(&node) {
                Some(p) if p.stopped_at == Some(lands_at) => p.stopped_at = None,
                Some(p) if !p.running() => *p = Playing::default(),
                Some(_) => {}
                None => {
                    playing.insert(node, Playing::default());
                }
            },
            Some(NamedAction::Update { node, .. }) => {
                playing.entry(node).or_default();
            }
            Some(NamedAction::Stop(node)) => {
                if let Some(p) = playing.get_mut(&node).filter(|p| p.running()) {
                    p.stopped_at = Some(lands_at);
                }
            }
            Some(NamedAction::Hush | NamedAction::Panic | NamedAction::Reset) => playing.clear(),
            _ => {}
        }
    }
}

/// Every name that `text` uses, directly or through the patterns, clips and functions the project defines
/// for them: what has to change for what `text` plays to change.
pub(super) fn depends_on(text: &str, project: &Project) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut queue = vec![text.to_string()];
    while let Some(next) = queue.pop() {
        for word in next.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
            if word.is_empty() || seen.iter().any(|s| s == word) {
                continue;
            }
            seen.push(word.to_string());
            for kind in ["bind", "clip", "def", "scene"] {
                if let Some(source) = project.text_of(&format!("{kind}:{word}")) {
                    queue.push(source.to_string());
                }
            }
        }
    }
    seen
}

/// Does `source` use any of `names` as a name?
pub(super) fn mentions_any(source: &str, names: &[String]) -> bool {
    source
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|word| names.iter().any(|n| n == word))
}
