//! Each report reader has its own cursor, including voices retained across definitions.
use crate::{ClientId, Session};
use niminal_engine::ops::Report;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};
struct Source {
    owner: u64,
    report: Arc<Report>,
    cursor: u64,
    dropped: u64,
}
struct Instance {
    name: String,
    graphs: Vec<Weak<niminal_engine::Graph>>,
}

#[derive(Default)]
pub(crate) struct Reports {
    sources: Vec<Source>,
    instances: BTreeMap<u64, Instance>,
}
impl Reports {
    pub fn register(&mut self, session: &Session) {
        self.sources.retain(|s| Arc::strong_count(&s.report) > 1);
        self.instances.retain(|_, instance| {
            instance.graphs.retain(|graph| graph.strong_count() > 0);
            !instance.graphs.is_empty()
        });
        let graphs = session
            .program()
            .instruments
            .iter()
            .map(|i| (&i.name, &i.graph))
            .chain(
                session
                    .program()
                    .nodes
                    .iter()
                    .filter_map(|t| t.def.chain.as_ref().map(|g| (&t.name, g))),
            );
        for (name, graph) in graphs {
            let owner = if let Some(rest) = name
                .strip_prefix("__routing_node_")
                .or_else(|| name.strip_prefix("__routing_fx_"))
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
            let instance = self
                .instances
                .entry(niminal_engine::instance_id(name))
                .or_insert_with(|| Instance {
                    name: public,
                    graphs: Vec::new(),
                });
            let weak = Arc::downgrade(graph);
            if !instance.graphs.iter().any(|g| g.ptr_eq(&weak)) {
                instance.graphs.push(weak);
            }
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
                        .map(|instance| instance.name.clone())
                        .unwrap_or_default()
                };
                result.push((owner,json!({"name":source.report.name,"source":name,"token":(token!=0).then_some(token),"sample":sample,"values":values,"dropped":source.dropped})));
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{At, Op};

    #[test]
    fn instance_names_survive_retained_graphs_and_are_pruned_after_release() {
        let mut session = Session::new(48_000., niminal_lang::Layout::Mono);
        let mut reports = Reports::default();
        session
            .send_events(
                0,
                vec![Op::Define {
                    source: "instr flat() { 0.25 }".into(),
                    at: At::Now,
                    dir: None,
                    tag: None,
                }],
            )
            .unwrap();
        session.process(1);
        reports.register(&session);
        let graph = session.program().instrument("flat").unwrap().graph.clone();
        let id = niminal_engine::instance_id("flat");
        session
            .send_events(
                0,
                vec![Op::Define {
                    source: "instr flat() { 0.5 }".into(),
                    at: At::Now,
                    dir: None,
                    tag: None,
                }],
            )
            .unwrap();
        session.process(1);
        reports.register(&session);
        // A held voice owns its old graph even after the replacement definition leaves the session.
        drop(session);
        let empty = Session::new(48_000., niminal_lang::Layout::Mono);
        reports.register(&empty);
        assert_eq!(reports.instances.get(&id).unwrap().name, "flat");
        drop(graph);
        reports.register(&empty);
        assert!(!reports.instances.contains_key(&id));
    }
}
