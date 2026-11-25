//! Cancelling queued events and tearing down a disconnected client.

use super::super::*;

impl Session {
    /// Take back this client's events that are still waiting, those with `tag` if one is given, at or after
    /// `from`. With `release`, held notes of the same tag that are already sounding are let go of at `from`.
    pub fn cancel_events(
        &mut self,
        client: ClientId,
        tag: Option<&str>,
        from: At,
        release: bool,
    ) -> Cancelled {
        self.sync_realtime();
        let from = self.resolve(from);
        self.log.push(LogEntry {
            sample: self.clock,
            input: LogInput::CancelEvents {
                client,
                tag: tag.map(str::to_owned),
                from,
                release,
            },
        });
        let mut done = Cancelled::default();
        let mut replan = false;
        let mine = |owner: Option<ClientId>, t: &Option<String>| {
            owner == Some(client) && tag.is_none_or(|x| t.as_deref() == Some(x))
        };

        // notes still to start
        let mut dropped_keys = Vec::new();
        let mut dropped_tokens = Vec::new();
        self.upcoming.retain(|u| {
            let drop = u.start >= from && mine(u.client, &u.tag);
            if drop {
                done.cancelled += 1;
                dropped_keys.extend(u.key);
                if let Some(activity) = &u.activity {
                    dropped_tokens.push(activity.token);
                }
            }
            !drop
        });
        replan |= done.cancelled > 0;
        self.voices.retain(|v| !dropped_keys.contains(&v.key));
        self.activities
            .retain(|n| !dropped_tokens.contains(&n.activity.token));

        // changes of a note's parameters, and releases, still to come
        let before = done.cancelled;
        let mut sets = std::mem::take(&mut self.voice_sets);
        sets.retain(|s| {
            let drop = !s.done && s.at >= from && mine(Some(s.client), &s.tag);
            done.cancelled += usize::from(drop);
            !drop
        });
        self.voice_sets = sets;
        let mut reopened = Vec::new();
        self.key_offs.retain(|k| {
            let drop = k.at >= from && k.at >= self.clock && mine(k.client, &k.tag);
            done.cancelled += usize::from(drop);
            if drop {
                reopened.push(k.key);
            }
            !drop
        });
        // a note whose release was taken back is held again
        for key in reopened {
            if let Some(i) = self.closed.iter().position(|c| c.key == key) {
                let entry = self.closed.remove(i);
                if !self
                    .voices
                    .iter()
                    .any(|v| v.client == entry.client && v.id == entry.id)
                {
                    self.voices.push(entry);
                }
            }
        }
        self.timed_panics.retain(|p| {
            let drop = !p.done && p.at >= from && mine(Some(p.client), &p.tag);
            done.cancelled += usize::from(drop);
            !drop
        });
        let mut nodes = std::mem::take(&mut self.node_sets);
        nodes.retain(|s| {
            let drop = !s.done && s.at >= from && mine(s.client, &s.tag);
            done.cancelled += usize::from(drop);
            !drop
        });
        self.chain_maps.retain(|s| {
            let drop = !s.done && s.at >= from && mine(Some(s.client), &s.tag);
            done.cancelled += usize::from(drop);
            !drop
        });
        self.buffer_binds.retain(|s| {
            let drop = !s.done && s.at >= from && mine(Some(s.client), &s.tag);
            done.cancelled += usize::from(drop);
            !drop
        });
        self.voice_maps.retain(|s| {
            let drop = !s.done && s.at >= from && mine(Some(s.client), &s.tag);
            done.cancelled += usize::from(drop);
            !drop
        });
        self.buffer_sets.retain(|s| {
            let drop = !s.done && s.at >= from && mine(Some(s.client), &s.tag);
            done.cancelled += usize::from(drop);
            !drop
        });
        self.node_sets = nodes;
        replan |= done.cancelled > before;

        // control changes the renderer has not carried out
        let clock = self.clock;
        let detached = self.detached;
        let mut withdrawn = Vec::new();
        let owners = std::mem::take(&mut self.control_owners);
        let (mine_owned, rest): (Vec<_>, Vec<_>) = owners
            .into_iter()
            .partition(|o| tag.is_none_or(|x| o.tag.as_deref() == Some(x)) && o.client == client);
        self.control_owners = rest;
        for owner in mine_owned {
            let Some(change) = self
                .control_changes
                .iter()
                .find(|c| c.serial == owner.serial)
            else {
                continue; // it has happened
            };
            let waiting = if detached {
                !change.applied
            } else {
                change.at > clock
            };
            if change.at >= from && waiting {
                withdrawn.push(owner.serial);
            } else {
                self.control_owners.push(owner);
            }
        }
        for serial in withdrawn {
            self.control_changes.retain(|c| c.serial != serial);
            if let Some(planner) = &mut self.live {
                let _ = planner.push_control(crate::controls::ControlMessage::Retract { serial });
            }
            done.cancelled += 1;
        }

        // changes of definitions that are still to land
        let landing: Vec<u64> = self
            .pending
            .iter()
            .filter(|p| p.lands_at >= from && mine(Some(p.meta.client), &p.meta.tag))
            .map(|p| p.id)
            .collect();
        for id in landing {
            let gone = self.cancel_change(id);
            done.cancelled += usize::from(gone);
            replan |= gone;
        }

        if release {
            let keys: Vec<u32> = self
                .voices
                .iter()
                .filter(|v| v.client == client && tag.is_none_or(|x| v.tag.as_deref() == Some(x)))
                .map(|v| v.key)
                .collect();
            self.voices.retain(|v| !keys.contains(&v.key));
            for key in keys {
                self.release_at(key, from.max(self.clock), None);
                done.released += 1;
            }
        }
        if replan {
            self.invalidate_windows();
        } else {
            self.wake_planner();
        }
        done
    }

    /// A client has gone: its held notes are let go of and its notes still waiting never sound.
    pub fn drop_client(&mut self, client: ClientId) {
        self.cancel_events(client, None, At::Now, true);
        if let Err(problems) = self.send_events(
            client,
            vec![Op::Forget {
                at: At::Now,
                tag: None,
            }],
        ) {
            self.notify(format!("routing cleanup failed: {}", problems[0].message));
        }
        self.client_samples.remove(&client);
        self.clocks.remove(&client);
        self.linked.retain(|_, link| link.client != client);
        self.buffer_budgets.remove(&client);
        self.hidden_buffers
            .retain(|name| definitions::owner(name) != client);
    }
}
