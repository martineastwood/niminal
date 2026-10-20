//! The live sequencer as a [`niminal_ws::Service`], so a WebSocket server can front it.

use std::sync::Mutex;

use niminal_daemon::ClientId;
use niminal_ws::{Begun, Service};
use serde_json::Value;

use crate::{CompileTicket, CompiledTicket, LiveServer, ParsedEval, Request};

impl Service for LiveServer {
    type Prepared = ParsedEval;
    type Job = Box<CompileTicket>;
    type Done = CompiledTicket;

    fn connect(&mut self) -> ClientId {
        LiveServer::connect(self)
    }
    fn disconnect(&mut self, client: ClientId) {
        LiveServer::disconnect(self, client);
    }
    fn notifications(&mut self) -> Vec<(ClientId, String)> {
        LiveServer::notifications(self)
    }
    fn prepare(text: &str) -> Option<ParsedEval> {
        LiveServer::prepare_eval(text)
    }
    fn begin(
        &mut self,
        client: ClientId,
        text: &str,
        prepared: Option<ParsedEval>,
    ) -> Begun<Self::Job> {
        match self.begin_prepared(client, text, prepared) {
            Request::Replies(replies) => Begun::Replies(replies),
            Request::Compile(ticket) => Begun::Job(ticket),
        }
    }
    fn compile(job: Self::Job) -> CompiledTicket {
        job.compile()
    }
    fn retry(&mut self, done: &CompiledTicket) -> Option<Self::Job> {
        LiveServer::retry(self, done).map(Box::new)
    }
    fn finish(&mut self, done: CompiledTicket) -> Vec<String> {
        LiveServer::finish(self, done)
    }
    fn preload(service: &Mutex<Self>, text: &str) {
        let Ok(request) = serde_json::from_str::<Value>(text) else {
            return;
        };
        if request.get("method").and_then(Value::as_str) != Some("eval") {
            return;
        }
        let params = &request["params"];
        let Some(source) = params.get("source").and_then(Value::as_str) else {
            return;
        };
        let samples = {
            let mut s = niminal_ws::lock(service);
            if let Some(dir) = params.get("dir").and_then(Value::as_str) {
                s.live_mut().set_sample_dir(std::path::Path::new(dir));
            }
            s.live().sample_files().clone()
        };
        samples.warm(source);
        // files named by modules the code uses
        let dir = params.get("dir").and_then(Value::as_str).unwrap_or(".");
        if let Ok(expanded) = niminal_compose::expand(source, std::path::Path::new(dir)) {
            samples.warm(&expanded.text[expanded.root_len..]);
        }
    }
}
