//! The server's audio host publishes devices and consumes explicit restart requests.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioSettings {
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub input: Option<String>,
    pub sample_rate: u32,
    pub channels: usize,
    #[serde(default)]
    pub buffer_size: Option<u32>,
    #[serde(default)]
    pub no_audio: bool,
}
#[derive(Default)]
struct State {
    devices: Value,
    status: Value,
    pending: Option<(u64, AudioSettings)>,
    next: u64,
}
#[derive(Clone, Default)]
pub struct AudioControl(Arc<Mutex<State>>);
impl AudioControl {
    pub fn publish_devices(&self, devices: Value) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).devices = devices;
    }
    pub fn publish_status(&self, status: Value) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).status = status;
    }
    pub fn devices(&self) -> Value {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .devices
            .clone()
    }
    pub fn status(&self) -> Value {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status
            .clone()
    }
    pub fn configure(&self, settings: AudioSettings, restart: bool) -> Result<Value, String> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.devices.is_null() {
            return Err("this engine has no audio host".into());
        }
        if !restart {
            return Err(
                "audio configuration resets all clients; set restart: true explicitly".into(),
            );
        }
        if !(8000..=384000).contains(&settings.sample_rate)
            || !(1..=16).contains(&settings.channels)
            || settings.buffer_size.is_some_and(|n| n == 0)
        {
            return Err("invalid audio rate, channel count or buffer size".into());
        }
        for (kind, selected) in [("outputs", &settings.output), ("inputs", &settings.input)] {
            if let Some(id) = selected
                && id != "default"
                && !state.devices[kind]
                    .as_array()
                    .is_some_and(|ds| ds.iter().any(|d| d["id"] == *id))
            {
                return Err(format!("unknown audio device `{id}`"));
            }
        }
        if state.pending.is_some() {
            return Err("an audio restart is already queued".into());
        }
        state.next += 1;
        let id = state.next;
        state.pending = Some((id, settings));
        Ok(json!({"id":id,"state":"queued","reset_clients":true}))
    }
    pub fn take_restart(&self) -> Option<(u64, AudioSettings)> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending
            .take()
    }
}
