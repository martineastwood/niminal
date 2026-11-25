//! Voice lifecycle, polyphony rules, controls, and buffer/mapping swaps on a [`Mixer`].

use std::sync::Arc;

use crate::graph::Graph;
use crate::voice::{GlideCurve, Voice, VoiceRule};

use super::{ChainFade, MAX_CHANNELS, Mixer, VoiceId};

impl Mixer {
    /// How long a sounding note is scheduled to last; see [`Voice::set_dur`].
    pub fn silence_owner(&mut self, owner: u64) {
        for node in &mut self.nodes {
            for (_, voice) in &mut node.voices {
                if voice.owner() == owner {
                    voice.set_activity_sample(self.buses.clock.sample);
                    voice.silence();
                }
            }
        }
    }

    pub fn attach_voice_activity(
        &mut self,
        id: VoiceId,
        activity: Arc<crate::Activity>,
        sample: u64,
    ) {
        activity.start(sample, id.serial);
        if let Some(node) = self.nodes.get_mut(id.node)
            && let Some((_, voice)) = node
                .voices
                .iter_mut()
                .find(|(s, v)| *s == id.serial || v.holds(id.serial))
        {
            voice.attach_activity(activity);
        }
    }

    pub fn finish_activities(&self, sample: u64, reason: u32) {
        for node in &self.nodes {
            for (_, voice) in &node.voices {
                voice.finish_activities(sample, reason);
            }
        }
    }

    pub fn set_voice_owner(&mut self, id: VoiceId, owner: u64) {
        if let Some(node) = self.nodes.get_mut(id.node)
            && let Some((_, voice)) = node.voices.iter_mut().find(|(s, _)| *s == id.serial)
        {
            voice.set_owner(owner);
        }
    }

    pub fn set_voice_dur(&mut self, id: VoiceId, seconds: f32) {
        if let Some(node) = self.nodes.get_mut(id.node)
            && let Some((_, voice)) = node
                .voices
                .iter_mut()
                .find(|(serial, _)| *serial == id.serial)
        {
            voice.set_dur(seconds);
        }
    }

    /// Admit an already constructed voice without allocating. Capacity is a
    /// hard limit; rejected voices remain owned by the caller for retirement.
    // the voice comes back in the error so that its owner can retire it without allocating
    #[allow(clippy::result_large_err)]
    pub fn note_prepared(&mut self, node: usize, voice: Voice) -> Result<VoiceId, Voice> {
        if self.muted
            || node >= self.nodes.len()
            || voice.channels() != self.channels
            || self.nodes[node].voices.len() == self.nodes[node].voices.capacity()
        {
            return Err(voice);
        }
        if let Some(group) = voice.group() {
            for (_, other) in self.nodes.iter_mut().flat_map(|t| t.voices.iter_mut()) {
                if other.group() == Some(group) {
                    other.choke();
                }
            }
        }
        let mut voice = voice;
        if let Some(rule) = voice.rule() {
            self.glide_into(node, &rule, &mut voice);
            self.make_room(node, &rule);
        }
        self.steal_if_full();
        let serial = self.next_serial;
        self.next_serial += 1;
        voice.start_activities(self.buses.clock.sample, serial);
        voice.start_note();
        if voice.rule().is_some_and(|r| r.legato) {
            voice.hold(serial);
        }
        self.nodes[node].admit(serial, voice);
        Ok(VoiceId { node, serial })
    }

    /// Start a note on `node`, setting `(parameter index, value)` pairs. The
    /// graph must produce as many channels as the mixer's master has.
    ///
    /// A note in a choke `group` ends every sounding note in the same group
    /// (on any node), with a short fade.
    pub fn note_on(
        &mut self,
        node: usize,
        graph: Arc<Graph>,
        params: &[(usize, f32)],
        group: Option<u64>,
    ) -> VoiceId {
        self.note_on_with(node, graph, params, group, None)
    }

    /// Like [`Mixer::note_on`], for an instrument that limits its own voices.
    pub fn note_on_with(
        &mut self,
        node: usize,
        graph: Arc<Graph>,
        params: &[(usize, f32)],
        group: Option<u64>,
        rule: Option<VoiceRule>,
    ) -> VoiceId {
        assert_eq!(
            graph.channels(),
            self.channels,
            "an instrument must output the master's channel count"
        );
        if group.is_some() {
            for (_, v) in self.nodes.iter_mut().flat_map(|t| t.voices.iter_mut()) {
                if v.group() == group {
                    v.choke();
                }
            }
        }
        let mut voice = Voice::new(graph, self.sample_rate);
        voice.set_group(group);
        voice.set_rule(rule);
        for &(index, value) in params {
            voice.set_param(index, value);
        }
        if let Some(rule) = &rule {
            if let Some(id) = self.merge_legato(node, &voice) {
                return id;
            }
            self.glide_into(node, rule, &mut voice);
            self.make_room(node, rule);
        }
        self.steal_if_full();
        let serial = self.next_serial;
        self.next_serial += 1;
        voice.start_activities(self.buses.clock.sample, serial);
        voice.start_note();
        if rule.is_some_and(|r| r.legato) {
            voice.hold(serial);
        }
        self.nodes[node].admit(serial, voice);
        VoiceId { node, serial }
    }

    /// A legato note that starts while its instrument's voice is held takes that voice over, so the
    /// envelopes carry on and only the pitch moves. The `note` it came from is only read, so a caller
    /// that must not free memory can retire it afterwards. Allocates nothing.
    pub fn merge_legato(&mut self, node: usize, note: &Voice) -> Option<VoiceId> {
        let rule = note.rule().filter(|r| r.legato && !self.muted)?;
        let serial = self.next_serial;
        let (_, voice) = self.nodes.get_mut(node)?.voices.iter_mut().find(|(_, v)| {
            v.is_legato_open() && v.rule().is_some_and(|r| r.instrument == rule.instrument)
        })?;
        voice.legato_on(serial, note);
        self.next_serial += 1;
        Some(VoiceId { node, serial })
    }

    /// A glissando into `voice`: it starts at the pitch the instrument's newest note had reached.
    fn glide_into(&self, node: usize, rule: &VoiceRule, voice: &mut Voice) {
        if rule.glide <= 0.0 {
            return;
        }
        let Some(node) = self.nodes.get(node) else {
            return;
        };
        let previous = node
            .voices
            .iter()
            .filter(|(_, v)| {
                !v.is_choked() && v.rule().is_some_and(|r| r.instrument == rule.instrument)
            })
            .max_by_key(|(serial, _)| *serial);
        if let Some((_, v)) = previous {
            voice.glide_from(v.pitch_now(), self.sample_rate);
        }
    }

    /// Make room for a note under `rule`: release the sounding note of the same pitch if asked, and
    /// end the oldest notes of the instrument until fewer than `max` are left. Allocates nothing.
    fn make_room(&mut self, node: usize, rule: &VoiceRule) {
        let Some(node) = self.nodes.get_mut(node) else {
            return;
        };
        let mine = |v: &Voice| v.rule().is_some_and(|r| r.instrument == rule.instrument);
        if rule.release_same_pitch {
            for (_, v) in node.voices.iter_mut() {
                if mine(v) && !v.is_released() && v.rule().is_some_and(|r| r.pitch == rule.pitch) {
                    v.release();
                }
            }
        }
        loop {
            // the voice to end: the quietest (the oldest of equals), or just the oldest
            let (mut count, mut ends) = (0usize, None::<(u64, f32)>);
            for (serial, v) in node.voices.iter() {
                if mine(v) && !v.is_choked() {
                    count += 1;
                    let level = if rule.steal_quietest { v.level() } else { 0.0 };
                    if ends.is_none_or(|(s, l)| level < l || (level == l && *serial < s)) {
                        ends = Some((*serial, level));
                    }
                }
            }
            let Some((ends, _)) = ends.filter(|_| count >= rule.max.max(1)) else {
                break;
            };
            if let Some((_, v)) = node.voices.iter_mut().find(|(serial, _)| *serial == ends) {
                v.choke();
            }
        }
    }

    fn steal_if_full(&mut self) {
        let count = self.active_voices();
        if count < self.max_voices {
            return;
        }
        let oldest = self
            .nodes
            .iter_mut()
            .flat_map(|t| t.voices.iter_mut())
            .filter(|(_, v)| !v.is_choked())
            .min_by_key(|(serial, _)| *serial);
        if let Some((_, voice)) = oldest {
            voice.choke();
            self.stolen += 1;
        }
    }

    pub fn set_control(&mut self, key: &str, start: f32, target: f32, frames: u64, elapsed: u64) {
        for node in &mut self.nodes {
            for (_, voice) in &mut node.voices {
                voice.set_control(key, start, target, frames, elapsed);
            }
            if let Some(chain) = &mut node.chain {
                let local = node
                    .def
                    .controls
                    .iter()
                    .find(|(public, _)| public == key)
                    .map_or(key, |(_, local)| local.as_str());
                // A template's private controls are changed through its instance keys only.
                if !node
                    .def
                    .controls
                    .iter()
                    .any(|(_, template)| template == key)
                    || node.def.controls.iter().any(|(public, _)| public == key)
                {
                    node.chain_quiet = 0;
                    chain.set_control(local, start, target, frames, elapsed);
                }
            }
        }
    }

    /// Place a sounding note among the speakers; see [`Voice::set_pan`].
    pub fn set_voice_pan(&mut self, id: VoiceId, gains: Option<[f32; MAX_CHANNELS]>) {
        if let Some(node) = self.nodes.get_mut(id.node)
            && let Some((_, voice)) = node
                .voices
                .iter_mut()
                .find(|(serial, _)| *serial == id.serial)
        {
            voice.set_pan(gains);
        }
    }

    pub fn set_voice_control(
        &mut self,
        id: VoiceId,
        key: &str,
        start: f32,
        target: f32,
        frames: u64,
        elapsed: u64,
    ) {
        if let Some(node) = self.nodes.get_mut(id.node)
            && let Some((_, voice)) = node
                .voices
                .iter_mut()
                .find(|(serial, _)| *serial == id.serial)
        {
            voice.set_control(key, start, target, frames, elapsed);
        }
    }

    /// Release a note. A voice that has already finished is ignored. Voices keep
    /// their id when they move between nodes (see [`Mixer::adopt`]), so the
    /// node it started on is only a hint.
    pub fn release(&mut self, id: VoiceId) {
        let ends = |(s, v): &(u64, Voice)| *s == id.serial || v.holds(id.serial);
        let sample = self.buses.clock.sample;
        let hinted = self
            .nodes
            .get_mut(id.node)
            .and_then(|t| t.voices.iter_mut().find(|n| ends(n)));
        if let Some((_, v)) = hinted {
            v.set_activity_sample(sample);
            v.note_off(id.serial);
            return;
        }
        for node in &mut self.nodes {
            if let Some((_, v)) = node.voices.iter_mut().find(|n| ends(n)) {
                v.set_activity_sample(sample);
                v.note_off(id.serial);
                return;
            }
        }
    }

    pub fn swap_voice_buffers(&mut self, id: VoiceId, key: u64, banks: &mut [crate::BufferBank]) {
        for node in &mut self.nodes {
            if let Some((_, voice)) = node
                .voices
                .iter_mut()
                .find(|(s, v)| *s == id.serial || v.holds(id.serial))
            {
                voice.swap_buffers(key, banks);
                return;
            }
        }
    }

    pub fn swap_chain_buffers(&mut self, node: usize, key: u64, banks: &mut [crate::BufferBank]) {
        if let Some(chain) = self.nodes.get_mut(node).and_then(|t| t.chain.as_mut()) {
            chain.swap_buffers(key, banks);
        }
    }

    pub fn chain_param_value(&self, node: usize, param: &str) -> Option<f32> {
        self.nodes.get(node)?.chain.as_ref()?.param_value(param)
    }

    pub fn swap_chain_mapping(
        &mut self,
        node: usize,
        param: &str,
        mapping: &mut Option<Box<crate::ParamMapping>>,
    ) {
        if let Some(chain) = self.nodes.get_mut(node).and_then(|t| t.chain.as_mut()) {
            chain.swap_mapping(param, mapping);
        }
    }

    pub fn swap_voice_mapping(
        &mut self,
        id: VoiceId,
        name: &str,
        mapping: &mut Option<Box<crate::ParamMapping>>,
    ) {
        let ends = |(s, v): &(u64, Voice)| *s == id.serial || v.holds(id.serial);
        for node in &mut self.nodes {
            if let Some((_, voice)) = node.voices.iter_mut().find(|v| ends(v)) {
                voice.swap_mapping(name, mapping);
                return;
            }
        }
    }

    pub fn set_voice_param_glide(
        &mut self,
        id: VoiceId,
        name: &str,
        value: f32,
        frames: u64,
        curve: GlideCurve,
    ) {
        let ends = |(s, v): &(u64, Voice)| *s == id.serial || v.holds(id.serial);
        for node in &mut self.nodes {
            if let Some((_, voice)) = node.voices.iter_mut().find(|v| ends(v)) {
                if let Some(index) = voice.param_index(name) {
                    voice.set_param_glide(index, value, frames, curve);
                }
                return;
            }
        }
    }

    /// Silence everything at once: stop every voice, clear every effect's
    /// memory (delay lines, reverb tails) and empty the buses.
    pub fn panic(&mut self) {
        let rate = self.sample_rate;
        for node in &mut self.nodes {
            node.voices.clear();
            node.fade = ChainFade::default();
            node.chain = node.def.chain.as_ref().map(|g| Voice::new(g.clone(), rate));
            node.chain_silenced = false;
            node.chain_quiet = 0;
        }
        self.buses.clear();
        self.muted = false;
    }
}
