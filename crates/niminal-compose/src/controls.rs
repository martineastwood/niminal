//! The values of the `ctl`s as the sequencer knows them, so a pattern can read one while it is played:
//! `motif.window(position, span)` follows the control `position` as it moves.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

#[derive(Default)]
struct Values {
    now: HashMap<String, f64>,
    /// What each started at, to tell a control that was redefined from one that was not.
    initial: HashMap<String, f64>,
}

/// The current value of every control, shared between the composition that was compiled and the live
/// sequencer that plays it. A pattern reads it when it is asked for its notes; the sequencer changes it as
/// the controls move, and no later than the notes it asks for.
#[derive(Clone, Default)]
pub struct ControlBank {
    values: Arc<Mutex<Values>>,
}

impl std::fmt::Debug for ControlBank {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.lock().now.iter()).finish()
    }
}

impl ControlBank {
    /// A bank holding each control at its starting value.
    pub fn new(controls: impl IntoIterator<Item = (String, f64)>) -> Self {
        let initial: HashMap<String, f64> = controls.into_iter().collect();
        ControlBank {
            values: Arc::new(Mutex::new(Values {
                now: initial.clone(),
                initial,
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Values> {
        // a panic elsewhere must not stop the music: the values are plain numbers, so whatever was left is usable
        self.values.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn has(&self, name: &str) -> bool {
        self.lock().now.contains_key(name)
    }

    pub fn get(&self, name: &str) -> Option<f64> {
        self.lock().now.get(name).copied()
    }

    /// Move a control. A name that is not a control is left out.
    pub fn set(&self, name: &str, value: f64) {
        if !value.is_finite() {
            return;
        }
        if let Some(v) = self.lock().now.get_mut(name) {
            *v = value;
        }
    }

    /// Bring the bank up to date with a piece's controls. One bank is kept for as long as the sequencer plays, so
    /// that clips that go on playing, and the pieces compiled later, read the same knobs: a control that is there
    /// and starts where it did keeps its value, and a new one, or one given another start, begins at its start.
    pub fn sync(&self, controls: impl IntoIterator<Item = (String, f64)>) {
        let mut values = self.lock();
        for (name, start) in controls {
            if values.initial.get(&name) != Some(&start) {
                values.initial.insert(name.clone(), start);
                values.now.insert(name, start);
            }
        }
    }
}
