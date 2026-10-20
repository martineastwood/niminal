//! Each report reader has its own cursor, including voices retained across definitions.
use crate::{ClientId, Session};
use niminal_engine::ops::Report;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
struct Source {
    owner: u64,
    report: Arc<Report>,
    cursor: u64,
    dropped: u64,
}
#[derive(Default)]
pub(crate) struct Reports {
    sources: Vec<Source>,
    instances: BTreeMap<u64, (ClientId, String)>,
}
impl Reports {
    pub fn register(&mut self, session: &Session) {
        self.sources.retain(|s| Arc::strong_count(&s.report) > 1);
        let mut graphs: Vec<_> = session
            .program()
            .instruments
            .iter()
            .map(|i| (&i.name, &i.graph))
            .collect();
        graphs.extend(
            session
                .program()
                .tracks
                .iter()
                .filter_map(|t| t.def.chain.as_ref().map(|g| (&t.name, g))),
        );
        for (name, graph) in graphs {
            let owner = if let Some(rest) = name
                .strip_prefix("__routing_node_")
                .or_else(|| name.strip_prefix("__routing_processor_"))
            {
                rest.split_once('_')
                    .and_then(|(id, _)| id.parse().ok())
                    .unwrap_or(0)
            } else {
                crate::session::definitions::owner(name)
            };
            let public = crate::session::definitions::local(owner, name)
                .unwrap_or(name)
                .to_string();
            self.instances
                .insert(niminal_engine::instance_id(name), (owner, public));
            graph.reports(&mut |report| {
                if !self.sources.iter().any(|s| Arc::ptr_eq(report, &s.report)) {
                    self.sources.push(Source {
                        owner,
                        report: report.clone(),
                        cursor: 0,
                        dropped: 0,
                    });
                }
            });
        }
    }
    pub fn poll(&mut self, session: &Session) -> Vec<(ClientId, Value)> {
        self.register(session);
        let mut result = Vec::new();
        for source in &mut self.sources {
            let (dropped, events) = source.report.read(&mut source.cursor);
            source.dropped += dropped;
            for (sample, token, instance, values) in events {
                let owner = source.owner;
                let name = if token != 0 {
                    session.note_target(token).unwrap_or_default()
                } else {
                    self.instances
                        .get(&instance)
                        .map(|(_, n)| n.clone())
                        .unwrap_or_default()
                };
                result.push((owner,json!({"name":source.report.name,"source":name,"token":(token!=0).then_some(token),"sample":sample,"values":values,"dropped":source.dropped})));
            }
        }
        result
    }
}
