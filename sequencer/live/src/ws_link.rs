//! The engine across a WebSocket: its JSON-RPC API on loopback. The live command and the stage both
//! reach the engine this way.

use niminal_server::{EngineError, EngineReply, EngineRequest, PROTOCOL_VERSION};
use niminal_ws::Client;
use serde_json::{Value, json};

use crate::EngineLink;

pub struct WsLink {
    client: Client,
    /// What the engine said when this link said hello: its audio format.
    pub hello: EngineReply,
    exit_when_lost: bool,
}

impl WsLink {
    /// Connect to the engine listening on `port` of this machine, saying hello as `name`.
    pub fn connect(port: u16, token: Option<&str>, name: &str) -> Result<WsLink, String> {
        let (mut client, reply) = Client::connect_as(port, token, name)?;
        let request = EngineRequest::Hello {
            protocol: PROTOCOL_VERSION,
            client: None,
            token: None,
        };
        let hello = EngineReply::from_json(&request, &reply)?;
        client.call("subscribe", json!({ "topics": ["notices"] }))?;
        Ok(WsLink {
            client,
            hello,
            exit_when_lost: false,
        })
    }

    /// End the program if the engine can't be reached, instead of reporting an error. There is no
    /// reconnecting: exiting closes our clients' connections so that an editor notices and starts a
    /// fresh sequencer against the restarted engine.
    pub fn exit_when_lost(mut self) -> WsLink {
        self.exit_when_lost = true;
        self
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
                if self.exit_when_lost {
                    eprintln!("the engine can't be reached: {message}");
                    std::process::exit(1);
                }
                Err(EngineError {
                    code: -32603,
                    message,
                    data: None,
                })
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
