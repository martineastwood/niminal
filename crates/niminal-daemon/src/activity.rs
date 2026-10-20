//! Public lifecycle reporting reads renderer-written receipts, never the planner clock.
use niminal_engine::{Activity, WAITING_SAMPLE};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
pub(crate) struct Note {
    pub activity: Arc<Activity>,
    id: Option<u64>,
    target: String,
    announced: [u64; 4],
}
impl Note {
    pub fn new(
        activity: Arc<Activity>,
        id: Option<u64>,
        target: String,
        _key: Option<u32>,
    ) -> Self {
        Self {
            activity,
            id,
            target,
            announced: [WAITING_SAMPLE; 4],
        }
    }
    fn times(&self) -> [u64; 4] {
        [
            self.activity.started.load(Ordering::Acquire),
            self.activity.released.load(Ordering::Acquire),
            self.activity.ended.load(Ordering::Acquire),
            self.activity.dropped.load(Ordering::Acquire),
        ]
    }
    fn json(&self) -> Value {
        json!({"token":self.activity.token,"id":self.id,"target":self.target,"started_at":sample(self.times()[0]),"released_at":sample(self.times()[1])})
    }
}
fn sample(n: u64) -> Option<u64> {
    (n != WAITING_SAMPLE).then_some(n)
}
fn reason(n: u32) -> &'static str {
    match n {
        1 => "choked",
        2 => "panic",
        3 => "invalid_dsp",
        4 => "graph_removed",
        5 => "capacity",
        _ => "finished",
    }
}
impl crate::Session {
    pub fn note_target(&self, token: u64) -> Option<String> {
        self.activities
            .iter()
            .find(|n| n.activity.token == token)
            .map(|n| {
                crate::session::definitions::local(n.activity.owner, &n.target)
                    .unwrap_or(&n.target)
                    .to_string()
            })
    }
    pub fn take_voice_updates(&mut self) -> Vec<(u64, Value)> {
        let mut updates = Vec::new();
        for note in &mut self.activities {
            let times = note.times();
            for i in 0..4 {
                if times[i] != WAITING_SAMPLE && note.announced[i] == WAITING_SAMPLE {
                    note.announced[i] = times[i];
                    let mut value = note.json();
                    value["state"] = json!(["started", "released", "ended", "dropped"][i]);
                    value["sample"] = json!(times[i]);
                    value["reason"] = if i >= 2 {
                        json!(reason(note.activity.reason.load(Ordering::Relaxed)))
                    } else {
                        Value::Null
                    };
                    crate::status::public_names(&mut value, note.activity.owner);
                    updates.push((note.activity.owner, value));
                }
            }
        }
        updates.sort_by_key(|(_, v)| v["sample"].as_u64());
        for (client, value) in &mut updates {
            let sequence = self.voice_sequence.entry(*client).or_default();
            *sequence += 1;
            value["sequence"] = json!(*sequence);
        }
        let mut terminal = self
            .activities
            .iter()
            .filter(|n| n.announced[2] != WAITING_SAMPLE || n.announced[3] != WAITING_SAMPLE)
            .count();
        self.activities.retain(|n| {
            if terminal > 256
                && (n.announced[2] != WAITING_SAMPLE || n.announced[3] != WAITING_SAMPLE)
            {
                terminal -= 1;
                false
            } else {
                true
            }
        });
        updates
    }
    pub fn active_voice_status(&self, client: u64) -> Value {
        let voices: Vec<_> = self
            .activities
            .iter()
            .filter(|n| {
                n.activity.owner == client
                    && n.times()[0] != WAITING_SAMPLE
                    && n.times()[2] == WAITING_SAMPLE
            })
            .map(|n| n.json())
            .collect();
        let mut result = json!({"sequence":self.voice_sequence.get(&client).copied().unwrap_or(0),"voices":voices});
        crate::status::public_names(&mut result, client);
        result
    }
}
