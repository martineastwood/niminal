//! How the sequencer reaches the engine: the same requests and replies whether the engine is across a
//! WebSocket or in this process.

use std::sync::{Arc, Mutex};

use niminal_server::{ClientId, EngineError, EngineReply, EngineRequest, EngineServer, PROTOCOL_VERSION};
use serde_json::Value;

pub trait EngineLink: Send {
    /// Ask the engine something and wait for the answer.
    fn request(&mut self, request: EngineRequest) -> Result<EngineReply, EngineError>;

    /// What the engine has pushed since this was last called, as `(method, params)`: `notice`, `meters`,
    /// `transport`.
    fn notifications(&mut self) -> Vec<(String, Value)>;
}

/// An engine in this process, which the link talks to without any text in between.
pub struct InProcess {
    server: Arc<Mutex<EngineServer>>,
    client: ClientId,
}

impl InProcess {
    pub fn client_id(&self) -> ClientId {
        self.client
    }
    pub fn new(server: Arc<Mutex<EngineServer>>) -> InProcess {
        let client = {
            let mut s = lock(&server);
            let client = s.connect_local();
            let hello = EngineRequest::Hello { protocol: PROTOCOL_VERSION, client: Some("niminal-live".into()), token: None };
            s.request(client, hello).expect("an engine in this process takes a hello");
            let topics = vec!["notices".to_string()];
            s.request(client, EngineRequest::Subscribe { topics }).expect("the notices topic exists");
            client
        };
        InProcess { server, client }
    }
}

fn lock(server: &Mutex<EngineServer>) -> std::sync::MutexGuard<'_, EngineServer> {
    server.lock().unwrap_or_else(|e| e.into_inner())
}

impl EngineLink for InProcess {
    fn request(&mut self, request: EngineRequest) -> Result<EngineReply, EngineError> {
        lock(&self.server).request(self.client, request)
    }

    fn notifications(&mut self) -> Vec<(String, Value)> {
        let mut server = lock(&self.server);
        server
            .notifications_for(self.client)
            .into_iter()
            .filter_map(|(_, text)| {
                let message: Value = serde_json::from_str(&text).ok()?;
                Some((message["method"].as_str()?.to_string(), message["params"].clone()))
            })
            .collect()
    }
}

impl Drop for InProcess {
    fn drop(&mut self) {
        lock(&self.server).disconnect(self.client);
    }
}
