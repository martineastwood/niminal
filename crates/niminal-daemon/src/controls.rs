//! Control trajectories shared by live and offline rendering.
use niminal_engine::{Mixer, Voice};
use niminal_lang::{Program, Unit};

pub(crate) const CAPACITY: usize = 1024;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Change {
    pub index: usize,
    pub unit: Unit,
    pub revision: u64,
    pub at: u64,
    pub value: f32,
    pub frames: u64,
    pub serial: u64,
    pub log_index: usize,
    pub applied: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct Applied {
    pub serial: u64,
    pub log_index: usize,
    pub at: u64,
}

#[derive(Clone)]
pub(crate) struct State {
    pub key: String,
    pub initial: f32,
    /// Zero inherits the running declaration; nonzero identifies an explicit redeclaration.
    pub revision: u64,
    pub start: f32,
    pub target: f32,
    pub at: u64,
    pub frames: u64,
}

impl State {
    pub fn value(&self, at: u64) -> f32 {
        if self.frames == 0 { return self.target; }
        let elapsed = at.saturating_sub(self.at).min(self.frames);
        (f64::from(self.start) + (f64::from(self.target) - f64::from(self.start))
            * (elapsed as f64 / self.frames as f64)) as f32
    }

    pub fn set(&mut self, at: u64, target: f32, frames: u64) {
        self.start = self.value(at);
        self.target = target;
        self.at = at;
        self.frames = frames;
    }
}

pub(crate) fn build(program: &Program) -> Vec<State> {
    program.controls.iter().map(|c| State { key: c.key.clone(), initial: c.initial,
        revision: 0, start: c.initial, target: c.initial, at: 0, frames: 0 }).collect()
}

/// The destination was constructed in the planner. Matching and numeric state
/// transfer allocate nothing; changed declarations glide to their new defaults.
pub(crate) fn adopt(new: &mut [State], old: &[State], program: &Program, at: u64, rate: f32) {
    for (state, def) in new.iter_mut().zip(&program.controls) {
        if let Some(previous) = old.iter().find(|p| p.key == state.key) {
            let changed = state.revision != 0 && state.revision != previous.revision;
            if state.revision == 0 { state.revision = previous.revision; }
            state.start = previous.start;
            state.target = previous.target;
            state.at = previous.at;
            state.frames = previous.frames;
            if changed || def.initial != previous.initial {
                state.set(at, def.initial, (def.smooth_seconds * f64::from(rate)).round() as u64);
            }
        }
    }
}

pub(crate) fn sync_mixer(states: &[State], mixer: &mut Mixer, at: u64) {
    for s in states {
        mixer.set_control(&s.key, s.start, s.target, s.frames, at.saturating_sub(s.at));
    }
}

pub(crate) fn sync_voice(states: &[State], voice: &mut Voice, at: u64) {
    for s in states {
        voice.set_control(&s.key, s.start, s.target, s.frames, at.saturating_sub(s.at));
    }
}

pub(crate) fn sync_note(states: &[State], mixer: &mut Mixer, id: niminal_engine::VoiceId, at: u64) {
    for s in states {
        mixer.set_voice_control(id, &s.key, s.start, s.target, s.frames, at.saturating_sub(s.at));
    }
}

pub(crate) fn mark(states: &mut [State], program: &Program, statements: &[niminal_lang::Statement], revision: u64) {
    for (state, def) in states.iter_mut().zip(&program.controls) {
        if statements.iter().any(|s| s.key.as_deref().and_then(|k| k.strip_prefix("ctl:")) == Some(&def.name)) {
            state.revision = revision;
        }
    }
}
