//! The sequencer's way to an engine across a WebSocket: the engine's JSON-RPC API on loopback.

use niminal_live::EngineLink;
use niminal_server::{EngineError, EngineReply, EngineRequest};
use serde_json::{Value, json};

use niminal_ws::Client;

pub struct WsLink {
    client: Client,
    /// What the engine said when this link said hello: its audio format.
    pub hello: EngineReply,
}

impl WsLink {
    /// Connect to the engine listening on `port` of this machine, saying hello as `nimlive`.
    pub fn connect(port: u16, token: Option<&str>) -> Result<WsLink, String> {
        let (mut client, reply) = Client::connect_as(port, token, "nimlive")?;
        let request = EngineRequest::Hello {
            protocol: niminal_server::PROTOCOL_VERSION,
            client: None,
            token: None,
        };
        let hello = EngineReply::from_json(&request, &reply)?;
        client.call("subscribe", json!({ "topics": ["notices"] }))?;
        Ok(WsLink { client, hello })
    }
}

impl EngineLink for WsLink {
    fn request(&mut self, request: EngineRequest) -> Result<EngineReply, EngineError> {
        let wire = serde_json::to_value(&request).map_err(|e| EngineError {
            code: -32603,
            message: e.to_string(),
            data: None,
        })?;
        let method = wire["method"].as_str().unwrap_or_default().to_string();
        let params = wire.get("params").cloned().unwrap_or_else(|| json!({}));
        match self.client.call_raw(&method, params) {
            Ok(Ok(result)) => {
                EngineReply::from_json(&request, &result).map_err(|message| EngineError {
                    code: -32603,
                    message,
                    data: None,
                })
            }
            Ok(Err(error)) => Err(EngineError::from_json(&error)),
            Err(message) => {
                // There is no reconnecting: exiting closes our clients' connections so that
                // an editor notices and starts a fresh sequencer against the restarted engine.
                eprintln!("the engine can't be reached: {message}");
                std::process::exit(1);
            }
        }
    }

    fn notifications(&mut self) -> Vec<(String, Value)> {
        self.client
            .take_notifications()
            .into_iter()
            .filter_map(|m| Some((m["method"].as_str()?.to_string(), m["params"].clone())))
            .collect()
    }
}
