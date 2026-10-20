//! Control-side waveform subscriptions. DSP only fills preallocated atomic storage.
use crate::{ClientId, Endpoint, Session};
use niminal_engine::Meter;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering},
};

struct Scope {
    meter: Arc<Meter>,
    endpoint: Endpoint,
    frames: usize,
    every: u64,
    last: Option<u64>,
}
#[derive(Default)]
pub(crate) struct Scopes {
    entries: BTreeMap<(ClientId, String), Scope>,
}
impl Scopes {
    pub fn create(
        &mut self,
        session: &Session,
        client: ClientId,
        id: String,
        endpoint: Endpoint,
        frames: usize,
        rate: f64,
    ) -> Result<Value, String> {
        if id.is_empty() || id.len() > 128 {
            return Err("scope id is 1–128 characters".into());
        }
        if !(1..=4096).contains(&frames) || !rate.is_finite() || !(0.5..=30.).contains(&rate) {
            return Err("scope frames are 1–4096 and rate is 0.5–30 updates/second".into());
        }
        if !self.entries.contains_key(&(client, id.clone()))
            && self.entries.keys().filter(|(c, _)| *c == client).count() >= 8
        {
            return Err("at most eight scopes per client".into());
        }
        let meter = session.endpoint_meter(client, &endpoint)?;
        meter.waveform().enabled.store(true, Ordering::Relaxed);
        self.remove(client, &id);
        meter.waveform().enabled.store(true, Ordering::Relaxed);
        self.entries.insert(
            (client, id.clone()),
            Scope {
                meter,
                endpoint,
                frames,
                every: (f64::from(session.sample_rate()) / rate).round() as u64,
                last: None,
            },
        );
        Ok(json!({"id":id}))
    }
    pub fn remove(&mut self, client: ClientId, id: &str) {
        if let Some(old) = self.entries.remove(&(client, id.to_string())) {
            if !self
                .entries
                .values()
                .any(|s| Arc::ptr_eq(&s.meter, &old.meter))
            {
                old.meter.waveform().enabled.store(false, Ordering::Relaxed);
            }
        }
    }
    pub fn disconnect(&mut self, client: ClientId) {
        let ids: Vec<_> = self
            .entries
            .keys()
            .filter(|(c, _)| *c == client)
            .map(|(_, id)| id.clone())
            .collect();
        for id in ids {
            self.remove(client, &id);
        }
    }
    pub fn poll(&mut self, session: &Session) -> Vec<(ClientId, Value)> {
        let now = session.clock();
        self.entries.iter_mut().filter_map(|((client,id),scope)| {
            // Replaced/freed endpoints must not keep showing their old waveform.
            let current=session.endpoint_meter(*client,&scope.endpoint).ok()?;
            if !Arc::ptr_eq(&current,&scope.meter) { return None; }
            if scope.last.is_some_and(|last|now<last+scope.every) { return None; }
            let (sample,channels,values)=scope.meter.waveform().read(scope.frames)?;
            scope.last=Some(now);
            Some((*client,json!({"id":id,"sample":sample,"sample_rate":session.sample_rate(),"channels":channels,"values":values})))
        }).collect()
    }
}
