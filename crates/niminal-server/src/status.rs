//! What the engine reports about itself.

use serde_json::{Value, json};

use crate::session::Session;

/// The engine's state: clock, definitions with their parameters, controls, realtime health and, for each
/// client, its held notes and how many of its events are waiting.
pub fn status_json(session: &Session) -> Value {
    let (instruments, tracks, buses) = session.definitions();
    json!({
        "clock": state_json(session),
        "routing": session.routing_status(),
        "routing_meters": session.routing_meters(false),
        "sample_rate": session.sample_rate(),
        "channels": session.channels(),
        "realtime": session.realtime_metrics().map(|m| json!({
            "prepared_until": session.prepared_until(),
            "late_events": m.late_events, "starved_frames": m.starved_frames,
            "capacity_drops": m.capacity_drops, "control_drops": m.control_drops, "retire_pressure": m.retire_pressure,
            "deadline_misses": m.deadline_misses, "max_render_micros": m.max_render_micros,
            "mixer_builds": m.mixer_builds,
        })),
        "controls": session.control_values().iter().map(|(name, value, target, unit)| {
            let display = |value: f32| if *unit == Some("db") { 20.0 * f64::from(value.max(f32::MIN_POSITIVE)).log10() } else { f64::from(value) };
            json!({ "name": name, "value": display(*value), "target": display(*target), "unit": unit })
        }).collect::<Vec<_>>(),
        "processing_latency": session.program().processing_latency(),
        "instruments": instruments.iter().map(|i| json!({
            "name": i.name,
            "members":i.members,
            "arrays": i.arrays,
            "buffers": i.resources,
            "params": i.params.iter().map(|p| json!({
                "name": p.name,
                "unit": p.unit.suffix(),
                "default": p.default,
                "range": p.range,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "tracks": tracks,
        "buses": buses,
        "clients": session.client_status().iter().map(|(client, voices, waiting)| json!({
            "id": client, "voices": voices, "waiting": waiting,
        })).collect::<Vec<_>>(),
    })
}

fn state_json(session: &Session) -> Value {
    let t = session.state();
    json!({
        "sample": t.sample, "seconds": t.seconds, "voices": t.voices,
    })
}

/// Public inspection is owner-scoped; health and clock remain engine-wide.
pub(crate) fn client_status_json(session: &Session, client: u64) -> Value {
    let mut status = status_json(session);
    for field in ["instruments", "controls"] {
        if let Some(entries) = status[field].as_array_mut() {
            entries.retain(|entry| {
                entry["name"]
                    .as_str()
                    .is_some_and(|name| crate::session::definitions::local(client, name).is_some())
            });
        }
    }
    for field in ["tracks", "buses"] {
        if let Some(entries) = status[field].as_array_mut() {
            entries.retain(|entry| {
                entry
                    .as_str()
                    .is_some_and(|name| crate::session::definitions::local(client, name).is_some())
            });
        }
    }
    for field in ["routing", "routing_meters"] {
        if let Some(entries) = status[field].as_array_mut() {
            entries.retain(|entry| entry["client"].as_u64() == Some(client));
        }
    }
    if let Some(entries) = status["clients"].as_array_mut() {
        entries.retain(|entry| entry["id"].as_u64() == Some(client));
    }
    public_names(&mut status, client);
    status
}
pub(crate) fn public_names(value: &mut Value, client: u64) {
    match value {
        Value::String(s) => {
            *s = s.replace(&format!("__client_{client}::"), "");
        }
        Value::Array(a) => {
            for v in a {
                public_names(v, client);
            }
        }
        Value::Object(o) => {
            for v in o.values_mut() {
                public_names(v, client);
            }
        }
        _ => {}
    }
}
