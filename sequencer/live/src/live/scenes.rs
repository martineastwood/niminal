//! Scenes play one at a time. Starting a scene switches to it: the running clips of the other scenes that it
//! does not use stop, its own start, and everything lands together, so there is no boundary to miss between a
//! stop and a start. Evaluating a scene that is playing again changes what is playing to match it.

use super::*;

/// The scenes the project defines, with their clips, in order.
type Scenes = Vec<(String, Vec<String>)>;

fn scenes_of(project: &Project) -> Scenes {
    project
        .entries
        .iter()
        .filter_map(|e| {
            let name = e.key.strip_prefix("scene:")?;
            Some((name.to_string(), niminal_compose::scene_clips(&e.text)))
        })
        .collect()
}

fn is_running(playing: &BTreeMap<String, Playing>, clip: &str) -> bool {
    playing.get(clip).is_some_and(Playing::running)
}

/// Is `name` the scene that is playing: all its clips run, and no bigger scene that holds them all is also
/// running in full (which would be the one that was started)?
fn is_current(name: &str, scenes: &Scenes, playing: &BTreeMap<String, Playing>) -> bool {
    let full = |clips: &[String]| !clips.is_empty() && clips.iter().all(|c| is_running(playing, c));
    let Some((_, mine)) = scenes.iter().find(|(n, _)| n == name) else {
        return false;
    };
    full(mine)
        && !scenes.iter().any(|(other, clips)| {
            other != name
                && clips.len() > mine.len()
                && mine.iter().all(|c| clips.contains(c))
                && full(clips)
        })
}

/// A command made for a scene, which lands when the statement it goes with does.
fn command(text: &str, like: &Statement, quantum: Option<Quantize>) -> Option<Statement> {
    let mut s = analyze(text).ok()?.into_iter().next()?;
    s.offset = like.offset;
    s.origin = like.origin;
    s.quantize = quantum;
    Some(s)
}

impl Live {
    /// Put the stops and starts that a scene needs next to the statements that ask for it: `chorus.start`
    /// also stops what the other scenes are playing that `chorus` does not use, and a scene that is playing
    /// and is defined again starts the clips it gained and stops the ones it lost. A clip counts as part of a
    /// scene whoever started it. Everything lands at the quantum of starting the scene.
    pub(super) fn expand_scenes(
        &self,
        statements: Vec<Statement>,
        fallback: Option<Quantize>,
    ) -> Vec<Statement> {
        let (project, playing) = self.projected();
        let before = scenes_of(&project);
        // what the scenes will be once this evaluation lands, for the starts it holds
        let mut after = before.clone();
        for s in &statements {
            if let Some(name) = s.key().and_then(|k| k.strip_prefix("scene:")) {
                let clips = niminal_compose::scene_clips(&s.text);
                match after.iter_mut().find(|(n, _)| n == name) {
                    Some(scene) => scene.1 = clips,
                    None => after.push((name.to_string(), clips)),
                }
            }
        }
        let mut out = Vec::new();
        for s in statements {
            let target = s.target().map(str::to_string);
            let starts_scene = s.kind() == StatementKind::Command
                && s.starts()
                && !s.has_at()
                && target
                    .as_deref()
                    .is_some_and(|t| after.iter().any(|(n, _)| n == t));
            if starts_scene {
                let name = target.expect("a scene start has a target");
                let quantum = self.quantum_of(&s, fallback);
                let (_, mine) = after
                    .iter()
                    .find(|(n, _)| *n == name)
                    .expect("the scene exists");
                let mut stops: Vec<&String> = Vec::new();
                for (other, clips) in &after {
                    if *other == name {
                        continue;
                    }
                    for clip in clips {
                        if is_running(&playing, clip)
                            && !mine.contains(clip)
                            && !stops.contains(&clip)
                        {
                            stops.push(clip);
                        }
                    }
                }
                for clip in stops {
                    out.extend(command(&format!("{clip}.stop"), &s, Some(quantum)));
                }
                out.push(s);
                continue;
            }
            let redefined = s
                .key()
                .and_then(|k| k.strip_prefix("scene:"))
                .map(str::to_string);
            let Some(name) = redefined else {
                out.push(s);
                continue;
            };
            let old = before
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, clips)| clips.clone());
            let Some(old) = old.filter(|_| is_current(&name, &before, &playing)) else {
                out.push(s);
                continue;
            };
            let new = niminal_compose::scene_clips(&s.text);
            // when starting this scene would land
            let Some(quantum) = command(&format!("{name}.start"), &s, None)
                .map(|probe| self.quantum_of(&probe, fallback))
            else {
                out.push(s);
                continue;
            };
            let (added, removed): (Vec<_>, Vec<_>) = (
                new.iter().filter(|c| !old.contains(c)).collect(),
                old.iter().filter(|c| !new.contains(c)).collect(),
            );
            out.push(s.clone());
            for clip in added {
                out.extend(command(&format!("{clip}.start"), &s, Some(quantum)));
            }
            for clip in removed {
                if is_running(&playing, clip) {
                    out.extend(command(&format!("{clip}.stop"), &s, Some(quantum)));
                }
            }
        }
        out
    }
}
