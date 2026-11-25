//! Applying validated plans to the session, plus voice parameter changes and releases.

use super::super::*;
use super::Plan;

impl Session {
    pub(super) fn apply_plans(
        &mut self,
        client: ClientId,
        ops: Vec<Op>,
        accepted: Vec<(crate::events::TransactionReceipt, Arc<AtomicU64>)>,
        plans: Vec<Plan>,
        clock: u64,
        has_changes: bool,
    ) -> Sent {
        let log_index = self.log.len();
        self.accept_transactions(&accepted, client);
        let mut sent = Sent {
            notes: Vec::new(),
            transactions: accepted.into_iter().map(|(r, _)| r).collect(),
            accepted: ops.len(),
            late: 0,
            first: None,
        };
        // definitions, which are queued already
        for op in &ops {
            let at = match op {
                Op::Define { at, .. } | Op::Forget { at, .. } | Op::Transaction { at, .. } => {
                    self.resolve(*at)
                }
                Op::Routing { at, edits, .. }
                    if !crate::routing::Routing::is_control_edit(edits) =>
                {
                    self.resolve(*at)
                }
                _ => continue,
            };
            sent.late += usize::from(at < clock);
            sent.first = Some(sent.first.map_or(at, |f| f.min(at)));
        }
        self.log.push(LogEntry {
            sample: clock,
            input: LogInput::Send { client, ops },
        });
        for plan in plans {
            let at = match plan {
                Plan::NodeBind {
                    id,
                    param,
                    resource,
                    bank,
                    count,
                    at,
                    tag,
                } => {
                    let bind = BufferBind {
                        param,
                        target: BufferTarget::Chain(id),
                        resource,
                        bank,
                        count,
                        at,
                        client,
                        tag,
                        done: false,
                    };
                    if self.live.is_some() {
                        self.fresh.push(realtime::Fresh::Bind(bind.clone()));
                    }
                    self.buffer_binds.push(bind);
                    at
                }
                Plan::ChainMap(map) => {
                    let at = map.at;
                    if self.live.is_some() {
                        self.fresh.push(realtime::Fresh::ChainMap(map.clone()));
                    }
                    self.chain_maps.push(map);
                    at
                }
                Plan::Bind {
                    id,
                    param,
                    resource,
                    bank,
                    count,
                    at,
                    tag,
                } => {
                    if let Some(entry) = self
                        .voices
                        .iter()
                        .find(|v| v.client == client && v.id == id)
                    {
                        let bind = BufferBind {
                            param,
                            target: BufferTarget::Voice(entry.key),
                            resource,
                            bank,
                            count,
                            at,
                            client,
                            tag,
                            done: false,
                        };
                        if self.live.is_some() {
                            self.fresh.push(realtime::Fresh::Bind(bind.clone()));
                        }
                        self.buffer_binds.push(bind);
                    }
                    at
                }
                Plan::Map {
                    id,
                    start,
                    param,
                    source,
                    channel,
                    scale,
                    offset,
                    tag,
                } => {
                    if let Some(entry) = self
                        .voices
                        .iter()
                        .find(|v| v.client == client && v.id == id)
                    {
                        let range = entry
                            .schema
                            .param(&param)
                            .ok()
                            .and_then(|(_, p)| p.range)
                            .map(|(lo, hi)| (lo as f32, hi as f32));
                        let set = VoiceMap {
                            range,
                            key: entry.key,
                            param,
                            source,
                            channel: channel - 1,
                            scale,
                            offset,
                            at: start.max(clock),
                            client,
                            tag,
                            done: false,
                        };
                        if self.live.is_some() {
                            self.fresh.push(realtime::Fresh::Map(set.clone()));
                        }
                        self.voice_maps.push(set);
                    }
                    start
                }
                Plan::Buffer { set, start } => {
                    if self.live.is_some() {
                        self.fresh.push(realtime::Fresh::Buffer(set.clone()));
                    }
                    self.buffer_sets.push(set);
                    start
                }
                Plan::Note {
                    op,
                    event,
                    instrument,
                    start,
                    dur,
                    id,
                    tag,
                } => {
                    let key = id.map(|_| self.new_key());
                    if let (Some(id), Some(key)) = (id, key) {
                        let schema = Arc::new(
                            self.program_at(start)
                                .instrument(&instrument)
                                .expect("validated instrument")
                                .clone(),
                        );
                        let destination = event
                            .target
                            .split_once('@')
                            .map(|(_, node)| node.to_string())
                            .or_else(|| {
                                event
                                    .target
                                    .starts_with("__routing_node_")
                                    .then(|| event.target.clone())
                            });
                        self.voices.push(VoiceEntry {
                            destination,
                            client,
                            id,
                            key,
                            schema,
                            tag: tag.clone(),
                        });
                    }
                    let token = self.next_note;
                    self.next_note += 1;
                    let activity = Arc::new(niminal_engine::Activity::new(token, client));
                    self.activities.push(crate::activity::Note::new(
                        activity.clone(),
                        id,
                        instrument.clone(),
                    ));
                    sent.notes
                        .push(crate::events::NoteReceipt { event: op, token });
                    self.add_explicit(
                        start.max(clock),
                        dur,
                        event,
                        key,
                        Some((client, tag)),
                        Some(activity),
                    );
                    start
                }
                Plan::Control {
                    name,
                    value,
                    frames,
                    at,
                    op,
                    tag,
                    now,
                } => {
                    let serial = self.next_control;
                    let log = (log_index, op);
                    match self.queue_control(&name, value, frames, at.max(clock), log, now) {
                        Ok(()) => self.control_owners.push(ControlOwner {
                            serial,
                            client,
                            tag,
                        }),
                        Err(e) => self.notify(e.into()),
                    }
                    at
                }
                Plan::Node {
                    key,
                    value,
                    frames,
                    at,
                    tag,
                } => {
                    let set = NodeSet {
                        key,
                        value,
                        frames,
                        at: at.max(clock),
                        client: Some(client),
                        tag,
                        done: false,
                    };
                    if self.live.is_some() {
                        self.fresh.push(realtime::Fresh::Node(set.clone()));
                    }
                    let place = self.node_sets.partition_point(|s| s.at <= set.at);
                    self.node_sets.insert(place, set);
                    at
                }
                Plan::Set {
                    id,
                    args,
                    frames,
                    at,
                    tag,
                } => {
                    self.set_voice(client, id, args, frames, at.max(clock), tag);
                    at
                }
                Plan::Panic { at, tag } => {
                    self.timed_panics.push(TimedPanic {
                        at: at.max(clock),
                        client,
                        tag,
                        done: false,
                    });
                    self.invalidate_windows();
                    at
                }
                Plan::Off { id, at, tag } => {
                    if let Some(i) = self
                        .voices
                        .iter()
                        .position(|v| v.client == client && v.id == id)
                    {
                        let entry = self.voices.remove(i);
                        self.release_at(entry.key, at.max(clock), Some((client, tag)));
                        self.closed.push(entry);
                    }
                    at
                }
            };
            if at < clock {
                sent.late += 1;
            }
            sent.first = Some(sent.first.map_or(at, |f| f.min(at)));
        }
        // a change that was asked for at once happens at once, and the renderer is told
        let waiting = self.pending.len();
        self.land_due(if self.detached {
            self.clock
        } else {
            self.clock.saturating_add(1)
        });
        if self.pending.len() != waiting {
            self.invalidate_live();
        } else if has_changes {
            self.invalidate_windows();
        }
        self.wake_planner();
        sent
    }

    /// Change parameters of the note a client holds by `id`: now, if it is sounding, or as it starts.
    fn set_voice(
        &mut self,
        client: ClientId,
        id: u64,
        args: Vec<(String, Value, f32)>,
        frames: u64,
        at: u64,
        tag: Option<String>,
    ) {
        let Some(entry) = self
            .voices
            .iter()
            .find(|v| v.client == client && v.id == id)
        else {
            return;
        };
        let key = entry.key;
        let pitch = entry.schema.pitch.clone();
        // a note that has not started by then starts with the value
        let waiting = self
            .upcoming
            .iter()
            .any(|u| u.key == Some(key) && u.start >= at);
        if waiting {
            for note in self.upcoming.iter_mut().filter(|u| u.key == Some(key)) {
                for (name, value, _) in &args {
                    note.event.args.insert(name.clone(), *value);
                }
            }
            // it may be prepared with the old value
            self.invalidate_live();
            return;
        }
        for (name, _, value) in args {
            let set = VoiceSet {
                curve: if name == pitch {
                    niminal_engine::GlideCurve::Pitch
                } else {
                    niminal_engine::GlideCurve::Linear
                },
                key,
                name,
                value,
                frames,
                at,
                done: false,
                client,
                tag: tag.clone(),
            };
            if self.live.is_some() {
                self.fresh.push(realtime::Fresh::Set(set.clone()));
            }
            self.voice_sets.push(set);
        }
    }

    /// Let go of the note a key holds, at sample `at`. A note that is not to start before then is played as a
    /// blip instead, long enough to hear.
    pub(in super::super) fn release_at(
        &mut self,
        key: u32,
        at: u64,
        owner: Option<(ClientId, Option<String>)>,
    ) {
        let stale = self.clock.saturating_sub(self.samples(2.0));
        self.key_offs.retain(|k| k.at > stale);
        let offs = &self.key_offs;
        self.closed.retain(|c| offs.iter().any(|k| k.key == c.key));
        let (client, tag) = owner.map_or((None, None), |(c, t)| (Some(c), t));
        self.key_offs.push(KeyOff {
            key,
            at,
            client,
            tag,
        });
        let blip = self.samples(0.01);
        let mut waiting = false;
        for note in self
            .upcoming
            .iter_mut()
            .filter(|u| u.key == Some(key) && u.start >= at)
        {
            note.key = None;
            note.dur = blip;
            waiting = true;
        }
        if waiting {
            // the note may be prepared already as a held one
            self.invalidate_live();
        } else {
            if self.live.is_some() {
                self.fresh.push(realtime::Fresh::Release { key, at });
            }
            self.wake_planner();
        }
    }
}
