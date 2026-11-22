use super::*;

impl Live {
    // ---- state ----------------------------------------------------------------

    pub fn pending(&self) -> Vec<PendingInfo> {
        let grid = self.grid();
        self.pending
            .iter()
            .filter(|p| !p.done)
            .map(|p| PendingInfo {
                id: p.id,
                lands_at: p.lands_at,
                in_samples: p.lands_at.saturating_sub(self.clock),
                position: grid.position(p.lands_at),
                changes: p.summary.clone(),
            })
            .collect()
    }

    pub fn transport(&self) -> Transport {
        let grid = self.grid();
        let (bar, beat) = grid.position(self.clock);
        Transport {
            sample: self.clock,
            seconds: self.seconds(self.clock),
            bar,
            beat,
            bpm: grid.bpm,
            voices: self.engine.as_ref().map_or(0, |e| e.voices),
            pending: self.pending.iter().filter(|p| !p.done).count(),
        }
    }

    /// The project's clips and what they are doing now.
    pub fn clips(&self) -> Vec<ClipStatus> {
        let now = self.seconds(self.clock);
        self.program
            .nodes
            .iter()
            .enumerate()
            .skip(1)
            // a node that only processes buses is not a clip to play
            .filter(|(_, t)| t.instrument.is_some())
            .map(|(i, t)| {
                let state = self.schedule.node_state(i, now);
                let running = state.clip.is_some();
                let record = self.playing.get(&t.name).filter(|p| running && p.running());
                // what it is playing now, or the definition it would play if started
                let clip = self
                    .schedule
                    .clip_at(i, now)
                    .or_else(|| self.program.clips.get(&t.name));
                let cycles = self
                    .schedule
                    .loop_position(i, now)
                    .map(|(cycles, _)| cycles);
                ClipStatus {
                    name: t.name.clone(),
                    instrument: t
                        .instrument
                        .map(|i| self.program.instruments[i].name.clone())
                        .unwrap_or_default(),
                    running,
                    muted: state.muted,
                    soloed: state.soloed,
                    edits: record.map(|p| p.edits.clone()).unwrap_or_default(),
                    length: clip.map_or(1.0, |c| c.length.to_f64()),
                    looped: clip.is_none_or(|c| c.looped()),
                    plays: clip.and_then(|c| c.plays),
                    endless: clip.is_some_and(|c| c.endless),
                    cycles,
                    lanes: clip
                        .map(|c| c.lane_preview(cycles.unwrap_or(0.0)))
                        .unwrap_or_default(),
                }
            })
            .collect()
    }

    /// The project's scenes, with how many of their clips are playing.
    pub fn scenes(&self) -> Vec<SceneStatus> {
        let running: Vec<String> = self
            .clips()
            .into_iter()
            .filter(|c| c.running)
            .map(|c| c.name)
            .collect();
        self.project
            .entries
            .iter()
            .filter_map(|e| {
                let name = e.key.strip_prefix("scene:")?;
                let clips = niminal_compose::scene_clips(&e.text);
                Some(SceneStatus {
                    name: name.to_string(),
                    running: clips.iter().filter(|c| running.contains(c)).count(),
                    clips,
                })
            })
            .collect()
    }

    /// The project's arrangements, and for each whether it is running, and where it has got to.
    pub fn arrangements(&self) -> Vec<ArrangementStatus> {
        let now = self.seconds(self.clock);
        self.defined("arrangement")
            .into_iter()
            .map(|name| {
                let known = self.program.arrangements.iter().find(|a| a.name == name);
                let started = self.arrangement_starts.get(&name).copied();
                let length = known.map_or(0.0, |a| a.length);
                let elapsed = started.map(|s| (now - s).max(0.0));
                let running = elapsed.is_some_and(|e| e < length);
                let section = known.filter(|_| running).and_then(|a| {
                    let e = elapsed?;
                    a.sections
                        .iter()
                        .find(|s| e >= s.start && e < s.end)
                        .map(|s| s.name.clone())
                });
                ArrangementStatus {
                    sections: known.map_or_else(Vec::new, |a| a.sections.clone()),
                    elapsed: elapsed.filter(|_| running),
                    name,
                    running,
                    length,
                    section,
                }
            })
            .collect()
    }

    /// The names of the project's definitions of one kind (`scene`, `clip`...), in order.
    pub fn defined(&self, kind: &str) -> Vec<String> {
        let prefix = format!("{kind}:");
        self.project
            .entries
            .iter()
            .filter_map(|e| e.key.strip_prefix(&prefix))
            .map(str::to_string)
            .collect()
    }

    /// Ask the engine for its meters, or not. Reading them resets them, so it is only done for someone who listens.
    pub fn set_metering(&mut self, on: bool) {
        let topics = vec!["meters".to_string()];
        let request = if on {
            EngineRequest::Subscribe { topics }
        } else {
            EngineRequest::Unsubscribe { topics }
        };
        let _ = self.link.request(request);
    }

    /// The loudest sample on each output channel since this was last called.
    pub fn take_peaks(&mut self) -> Vec<f32> {
        std::mem::replace(&mut self.peaks, vec![0.0; self.layout.channels()])
    }

    /// The loudest sample of each node and each bus since this was last called, by the name the project gives it.
    pub fn take_levels(&mut self) -> Levels {
        // the names stay, at silence: the engine reports less often than this is read, and a name that
        // came and went with each reading would make a meter flicker in and out
        let silence = |peaks: &mut BTreeMap<String, f32>| {
            let read = peaks.clone();
            peaks.values_mut().for_each(|peak| *peak = 0.0);
            read
        };
        Levels {
            nodes: silence(&mut self.node_peaks),
            buses: silence(&mut self.bus_peaks),
        }
    }

    /// The engine's own account of itself, for a status that wants it. An error means the engine could not be
    /// reached.
    pub fn engine_status(&mut self) -> Result<serde_json::Value, String> {
        match self.link.request(EngineRequest::Status {}) {
            Ok(EngineReply::Json(mut v)) => {
                self.tidy_controls(&mut v);
                Ok(v)
            }
            Ok(_) => Err("the engine answered a status request with something else".into()),
            Err(error) => Err(error.message),
        }
    }

    /// Keep this client's controls under the names the code gives them, with what a widget needs about each.
    fn tidy_controls(&self, v: &mut serde_json::Value) {
        let client = v["client_id"].as_u64().unwrap_or(0);
        if let Some(controls) = v["controls"].as_array_mut() {
            controls.retain_mut(|control| {
                let Some(name) = control["name"]
                    .as_str()
                    .and_then(|n| self.control_name(n, client))
                else {
                    return false;
                };
                self.describe_control(control, &name);
                control["name"] = serde_json::Value::String(name);
                true
            });
        }
    }

    /// Add to a control's status what a widget needs: the `range` it moves over in the units `value` is
    /// shown in (null when the code gives none), whether it moves on a `log` scale, what drives it
    /// (`source`, such as `midi.cc(21)`) and whether it is a `ctl` or the parameter of an effect.
    fn describe_control(&self, control: &mut serde_json::Value, name: &str) {
        let def = self.program.controls.iter().find(|c| c.name == name);
        let binding = self.program.input(name);
        let db = control["unit"] == "db";
        let shown = |v: f64| {
            if db {
                20.0 * v.max(f64::MIN_POSITIVE).log10()
            } else {
                v
            }
        };
        let range = binding
            .map(|b| (f64::from(b.lo), f64::from(b.hi)))
            .or_else(|| def.and_then(|d| d.range));
        control["range"] = match range {
            Some((lo, hi)) => serde_json::json!([shown(lo), shown(hi)]),
            None => serde_json::Value::Null,
        };
        control["log"] = serde_json::json!(
            control["unit"] == "hz" && range.is_some_and(|(lo, hi)| lo > 0.0 && hi > 0.0)
        );
        control["source"] = binding.map_or(serde_json::Value::Null, |b| b.source.key().into());
        control["kind"] = if name.contains("::") { "fx" } else { "ctl" }.into();
    }
}

/// The peaks of the nodes or buses in an engine `meters` report, as one level each: the loudest channel.
pub(super) fn note_peaks(into: &mut BTreeMap<String, f32>, entries: &serde_json::Value) {
    // what the engine no longer lists is gone
    let listed: Vec<String> = entries
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["id"].as_str().and_then(routing_name))
        .collect();
    into.retain(|name, _| listed.contains(name));
    for entry in entries.as_array().into_iter().flatten() {
        let Some(name) = entry["id"].as_str().and_then(routing_name) else {
            continue;
        };
        let peak = entry["peaks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_f64)
            .fold(0.0, f64::max) as f32;
        let level = into.entry(name).or_insert(0.0);
        *level = level.max(peak);
    }
}

/// The name a routing id was made from: the inverse of [`routing_id`].
pub(super) fn routing_name(id: &str) -> Option<String> {
    let hex = id.strip_prefix("live_")?;
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

pub(crate) fn routing_id(name: &str) -> String {
    format!(
        "live_{}",
        name.as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

#[cfg(test)]
mod level_tests {
    use super::*;

    #[test]
    fn routing_ids_name_their_nodes_again() {
        assert_eq!(
            routing_name(&routing_id("bass line")),
            Some("bass line".into())
        );
        assert_eq!(routing_name("sum"), None);
    }

    #[test]
    fn a_nodes_level_is_its_loudest_channel() {
        let mut levels = BTreeMap::new();
        let report = serde_json::json!([{ "id": routing_id("kick"), "peaks": [0.25, 0.5] }]);
        note_peaks(&mut levels, &report);
        note_peaks(
            &mut levels,
            &serde_json::json!([{ "id": routing_id("kick"), "peaks": [0.1, 0.2] }]),
        );
        assert_eq!(levels["kick"], 0.5);
    }
}
