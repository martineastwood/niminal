//! Rich parameter updates use the same scalar event path, validation and replay as other controls.
use super::*;

impl Session {
    pub(super) fn expand_parameter_events(
        &self,
        client: ClientId,
        ops: Vec<Op>,
    ) -> Result<Vec<Op>, Vec<Problem>> {
        let mut schemas: HashMap<u64, Arc<niminal_lang::Instrument>> = self
            .voices
            .iter()
            .filter(|v| v.client == client)
            .map(|v| (v.id, v.schema.clone()))
            .collect();
        let mut expanded = Vec::new();
        for (event, op) in ops.into_iter().enumerate() {
            let fail = |message: String| {
                let mut p = Problem::plain(message);
                p.event = Some(event);
                vec![p]
            };
            match op {
                Op::Note {
                    id: Some(id),
                    ref target,
                    at,
                    ..
                } => {
                    let program = self.program_at(self.resolve(at));
                    let local = if self.routing_at(self.resolve(at)).has_node(client, target) {
                        crate::routing::node_name(client, target)
                    } else {
                        target.clone()
                    };
                    let target = &local;
                    let instrument = program.instrument(target).or_else(|| {
                        program
                            .track_index(target)
                            .and_then(|t| program.tracks[t].instrument)
                            .map(|i| &program.instruments[i])
                    });
                    if let Some(i) = instrument {
                        schemas.insert(id, Arc::new(i.clone()));
                    }
                    expanded.push(op);
                }
                Op::Array {
                    id,
                    at,
                    param,
                    values,
                    glide,
                    tag,
                } => {
                    let schema = schemas
                        .get(&id)
                        .ok_or_else(|| fail(format!("no note with id {id} is sounding")))?;
                    let paths = schema
                        .arrays
                        .get(&param)
                        .ok_or_else(|| fail(format!("`{param}` is not a live array parameter")))?;
                    if paths.len() != values.len() {
                        return Err(fail(format!(
                            "array `{param}` needs {} values",
                            paths.len()
                        )));
                    }
                    expanded.push(Op::Set {
                        id,
                        at,
                        args: paths.iter().cloned().zip(values).collect(),
                        glide,
                        tag,
                    });
                }
                Op::BindBuffer {
                    id,
                    at,
                    param,
                    buffer,
                    tag,
                } => {
                    let schema = schemas
                        .get(&id)
                        .ok_or_else(|| fail(format!("no note with id {id} is sounding")))?;
                    let choices = schema
                        .resources
                        .get(&param)
                        .ok_or_else(|| fail(format!("`{param}` is not a live buffer parameter")))?;
                    let index = choices.iter().position(|n| *n == buffer).ok_or_else(|| {
                        fail(format!("buffer `{buffer}` was not prepared for this voice"))
                    })?;
                    expanded.push(Op::Set {
                        id,
                        at,
                        args: [(param, Value::Num(index as f64))].into(),
                        glide: None,
                        tag,
                    });
                }
                Op::Off { id, .. } => {
                    schemas.remove(&id);
                    expanded.push(op);
                }
                _ => expanded.push(op),
            }
        }
        Ok(expanded)
    }
}

pub(super) fn prepare_mapping(
    program: &Program,
    set: &VoiceMap,
) -> Option<Box<niminal_engine::ParamMapping>> {
    let source = set.source.as_ref()?;
    let mapping = if let Some(control) = program.controls.iter().find(|c| c.name == *source) {
        niminal_engine::ParamMapping::control(
            control.key.clone(),
            control.initial,
            set.scale,
            set.offset,
        )
    } else {
        let bus = program.buses.iter().position(|n| n == source)?;
        niminal_engine::ParamMapping::bus(bus, set.channel, set.scale, set.offset)
    };
    Some(Box::new(mapping.with_range(set.range)))
}
impl Session {
    pub(super) fn apply_voice_maps(&mut self) {
        for set in &mut self.voice_maps {
            if set.done || set.at > self.clock {
                continue;
            }
            if let Some((_, id, _)) = self.sounding.held.iter().find(|h| h.2 == set.key) {
                let mut mapping = prepare_mapping(&self.program, set);
                self.mixer.swap_voice_mapping(*id, &set.param, &mut mapping);
                crate::controls::sync_note(&self.controls, &mut self.mixer, *id, self.clock);
            }
            set.done = true;
        }
        let stale = self.clock.saturating_sub(self.sample_rate as u64 * 2);
        self.voice_maps.retain(|s| !s.done || s.at > stale);
    }
}
