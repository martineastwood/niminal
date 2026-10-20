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
                    let local = if self
                        .routing_at(self.resolve(at))
                        .has_node(client, definitions::local(client, target).unwrap_or(target))
                    {
                        crate::routing::node_name(
                            client,
                            definitions::local(client, target).unwrap_or(target),
                        )
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

impl Session {
    pub(super) fn apply_buffer_binds(&mut self) {
        for bind in &mut self.buffer_binds {
            if !bind.done && bind.at <= self.clock {
                if let Some(track) = &bind.target {
                    if let Some(index) = self.program.track_index(track) {
                        self.mixer
                            .swap_chain_buffers(index, bind.resource, &mut bind.banks());
                    }
                } else if let Some((_, id, _)) = self.sounding.held.iter().find(|h| h.2 == bind.key)
                {
                    self.mixer
                        .swap_voice_buffers(*id, bind.resource, &mut bind.banks());
                }
                bind.done = true;
            }
        }
        let stale = self.clock.saturating_sub(self.sample_rate as u64 * 2);
        self.prune_buffer_binds(stale);
    }
}

impl Session {
    pub(super) fn apply_chain_maps(&mut self) {
        for map in &mut self.chain_maps {
            if !map.mapping.done && map.mapping.at <= self.clock {
                if let Some(track) = self.program.track_index(&map.track()) {
                    if map.mapping.source.is_none()
                        && let Some(value) = self.mixer.chain_param_value(track, &map.param)
                            && let Some(control) =
                                self.controls.iter_mut().find(|c| c.key == map.param)
                            {
                                control.set(self.clock, value, 0);
                            }
                    let mut prepared = prepare_mapping(&self.program, &map.mapping);
                    self.mixer
                        .swap_chain_mapping(track, &map.param, &mut prepared);
                    crate::controls::sync_mixer(&self.controls, &mut self.mixer, self.clock);
                }
                map.mapping.done = true;
            }
        }
        self.prune_chain_maps();
    }
}

impl Session {
    pub(super) fn prune_buffer_binds(&mut self, stale: u64) {
        let mut latest = std::collections::BTreeMap::new();
        for (index, bind) in self.buffer_binds.iter().enumerate() {
            if bind.done && bind.target.is_some() {
                let key = (bind.client, bind.target.clone(), bind.param.clone());
                if latest
                    .get(&key)
                    .is_none_or(|old: &usize| self.buffer_binds[*old].at <= bind.at)
                {
                    latest.insert(key, index);
                }
            }
        }
        let mut index = 0;
        self.buffer_binds.retain(|b| {
            let target_exists = b
                .target
                .as_ref()
                .is_none_or(|track| self.program.track_index(track).is_some());
            let keep = !b.done
                || (target_exists
                    && (b.at > stale
                        || (b.target.is_some()
                            && latest.get(&(b.client, b.target.clone(), b.param.clone()))
                                == Some(&index))));
            index += 1;
            keep
        });
    }
    pub(super) fn prune_chain_maps(&mut self) {
        let mut latest = std::collections::BTreeMap::new();
        for (i, map) in self.chain_maps.iter().enumerate() {
            if map.mapping.done {
                let key = (map.client, map.node.clone(), map.param.clone());
                if latest
                    .get(&key)
                    .is_none_or(|old: &usize| self.chain_maps[*old].mapping.at <= map.mapping.at)
                {
                    latest.insert(key, i);
                }
            }
        }
        let mut index = 0;
        self.chain_maps.retain(|map| {
            let keep = !map.mapping.done
                || (self.program.track_index(&map.track()).is_some()
                    && latest.get(&(map.client, map.node.clone(), map.param.clone()))
                        == Some(&index));
            index += 1;
            keep
        });
    }
    pub(super) fn restore_chain_bindings(&mut self) {
        let mut latest = std::collections::BTreeMap::new();
        for map in &self.chain_maps {
            if map.mapping.at <= self.clock {
                let key = (map.client, map.node.clone(), map.param.clone());
                if latest
                    .get(&key)
                    .is_none_or(|old: &&super::ChainMap| old.mapping.at <= map.mapping.at)
                {
                    latest.insert(key, map);
                }
            }
        }
        for map in latest.values() {
            if map.mapping.source.is_some()
                && let Some(track) = self.program.track_index(&map.track()) {
                    let mut mapping = prepare_mapping(&self.program, &map.mapping);
                    self.mixer
                        .swap_chain_mapping(track, &map.param, &mut mapping);
                }
        }
        let mut latest = std::collections::BTreeMap::new();
        for bind in &self.buffer_binds {
            if bind.at <= self.clock && bind.target.is_some() {
                let key = (bind.client, bind.target.clone(), bind.param.clone());
                if latest
                    .get(&key)
                    .is_none_or(|old: &&super::BufferBind| old.at <= bind.at)
                {
                    latest.insert(key, bind);
                }
            }
        }
        for bind in latest.values() {
            let track = bind.target.as_ref().unwrap();
            let Some(index) = self.program.track_index(track) else {
                continue;
            };
            let Some(node) = track
                .strip_prefix("__routing_processor_")
                .and_then(|r| r.split_once('_'))
                .map(|(_, id)| id)
            else {
                continue;
            };
            if let Ok((key, count, _)) = self.routing.buffer_binding(bind.client, node, &bind.param)
            {
                self.mixer
                    .swap_chain_buffers(index, key, &mut vec![bind.bank.clone(); count]);
            }
        }
        crate::controls::sync_mixer(&self.controls, &mut self.mixer, self.clock);
    }
}
