//! The sequencer's connection to an engine in the same process, using its public requests and replies.

use std::sync::{Arc, Mutex};

use niminal_server::{
    ClientId, EngineError, EngineReply, EngineRequest, EngineServer, PROTOCOL_VERSION,
};
use niminal_ws::lock;
use serde_json::Value;

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
            let hello = EngineRequest::Hello {
                protocol: PROTOCOL_VERSION,
                client: Some("niminal-sequencer".into()),
                token: None,
            };
            s.request(client, hello)
                .expect("an engine in this process takes a hello");
            let topics = vec!["notices".to_string()];
            s.request(client, EngineRequest::Subscribe { topics })
                .expect("the notices topic exists");
            client
        };
        InProcess { server, client }
    }

    /// Ask the engine something and wait for the answer.
    pub fn request(&mut self, request: EngineRequest) -> Result<EngineReply, EngineError> {
        lock(&self.server).request(self.client, request)
    }

    /// What the engine has pushed since this was last called, as `(method, params)`.
    pub fn notifications(&mut self) -> Vec<(String, Value)> {
        lock(&self.server).notifications_for(self.client)
    }
}

impl Drop for InProcess {
    fn drop(&mut self) {
        lock(&self.server).disconnect(self.client);
    }
}
