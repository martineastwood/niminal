//! What the engine reports about itself.

use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{Value, json};

use crate::session::Session;

fn visible_name(name: &str, client: Option<u64>) -> Option<&str> {
    match client {
        Some(client) => crate::session::definitions::local(client, name),
        None => Some(name),
    }
}

fn controls_json(values: &[(String, f32, f32, Option<&str>)], client: Option<u64>) -> Vec<Value> {
    values.iter().filter_map(|(name, value, target, unit)| {
        let name = visible_name(name, client)?;
        let display = |value: f32| if *unit == Some("db") {
            20.0 * f64::from(value.max(f32::MIN_POSITIVE)).log10()
        } else { f64::from(value) };
        Some(json!({ "name": name, "value": display(*value), "target": display(*target), "unit": unit }))
    }).collect()
}

/// The engine's state: clock, definitions with their parameters, controls, realtime health and, for each
/// client, its held notes and how many of its events are waiting.
pub fn status_json(session: &Session) -> Value {
    inspection_json(session, None)
}

fn inspection_json(session: &Session, client: Option<u64>) -> Value {
    let program = session.program();
    let values = session.control_values();
    let mut routing = session.routing.status(&values, client);
    if let Some(client) = client {
        public_names(&mut routing, client);
    }
    json!({
        "clock": state_json(session),
        "routing": routing,
        "routing_meters": session.routing.meters(false, client),
        "sample_rate": session.sample_rate(),
        "channels": session.channels(),
        "realtime": session.realtime_metrics().map(|m| json!({
            "prepared_until": session.prepared_until(),
            "late_events": m.late_events, "starved_frames": m.starved_frames,
            "capacity_drops": m.capacity_drops, "control_drops": m.control_drops, "retire_pressure": m.retire_pressure,
            "deadline_misses": m.deadline_misses, "max_render_micros": m.max_render_micros,
            "mixer_builds": m.mixer_builds, "planner_panics": m.planner_panics,
        })),
        "controls": controls_json(&values, client),
        "processing_latency": session.program().processing_latency(),
        "instruments": program.instruments.iter().filter_map(|i| {
            let name = visible_name(&i.name, client)?;
            let mut instrument = json!({
            "name": name,
            "members":i.members,
            "arrays": i.arrays,
            "buffers": i.resources,
            "params": i.params.iter().map(|p| json!({
                "name": p.name,
                "unit": p.unit.suffix(),
                "default": p.default,
                "range": p.range,
            })).collect::<Vec<_>>(),
        });
            if let Some(client) = client { public_names(&mut instrument, client); }
            Some(instrument)
        }).collect::<Vec<_>>(),
        "nodes": program.nodes.iter().skip(1).filter_map(|n| visible_name(&n.name, client)).collect::<Vec<_>>(),
        "load": load_json(session, client),
        "buses": program.buses.iter().filter_map(|name| visible_name(name, client)).collect::<Vec<_>>(),
        "clients": session.client_status().iter().filter(|(id, _, _)| client.is_none_or(|client| client == *id)).map(|(client, voices, waiting)| json!({
            "id": client, "voices": voices, "waiting": waiting,
        })).collect::<Vec<_>>(),
    })
}

/// What each node costs to render, as a share of the time one block has: `voices` is the work of the
/// node's voices (summed over every thread that helped, so it can pass 1), `chain` its effect chain.
fn load_json(session: &Session, client: Option<u64>) -> Value {
    let program = session.program();
    let budget = niminal_engine::block_size() as f64 / f64::from(session.sample_rate()) * 1e9;
    let share = |nanos: &AtomicU32| f64::from(nanos.load(Ordering::Relaxed)) / budget;
    program
        .nodes
        .iter()
        .zip(&program.profile.nodes)
        .filter(|(node, _)| !node.name.is_empty() && !node.name.contains("__routing_"))
        .filter_map(|(node, load)| {
            let name = visible_name(&node.name, client)?;
            Some(json!({
                "name": name,
                "voices": load.voices.load(Ordering::Relaxed),
                "voice_cpu": share(&load.voice_nanos),
                "chain_cpu": share(&load.chain_nanos),
            }))
        })
        .collect()
}

fn state_json(session: &Session) -> Value {
    let t = session.state();
    json!({
        "sample": t.sample, "seconds": t.seconds, "voices": t.voices,
    })
}

/// Public inspection is owner-scoped; health and clock remain engine-wide.
pub(crate) fn client_status_json(session: &Session, client: u64) -> Value {
    inspection_json(session, Some(client))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{At, Op};

    #[test]
    fn client_inspection_filters_before_serializing_and_preserves_public_names() {
        let mut session = Session::new(48_000., niminal_lang::Layout::Mono);
        for client in [1, 2] {
            session.send_events(client, vec![Op::Define {
                source: "ctl level = 0.5\ninstr tone() { level }\nnode lead { instrument = tone }\nbus send".into(),
                at: At::Now, dir: None, tag: None,
            }]).unwrap();
        }
        session.process(1);
        let global = status_json(&session);
        let own = client_status_json(&session, 1);
        for field in ["instruments", "controls", "load"] {
            let mut expected = json!(
                global[field]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|entry| {
                        crate::session::definitions::local(1, entry["name"].as_str().unwrap())
                            .is_some()
                    })
                    .collect::<Vec<_>>()
            );
            public_names(&mut expected, 1);
            assert_eq!(own[field], expected, "{field}");
        }
        assert_eq!(own["nodes"], json!(["lead"]));
        assert_eq!(own["buses"], json!(["send"]));
        assert!(!own.to_string().contains("__client_"));
    }
}
