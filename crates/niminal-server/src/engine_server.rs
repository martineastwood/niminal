//! The engine's public API: JSON-RPC 2.0 over whatever carries the messages (a WebSocket, in practice),
//! and the same requests as plain values for a client that lives in the same process.
//!
//! An [`EngineServer`] owns a [`Session`] and speaks the protocol to any number of clients. A client sends
//! timestamped events and reads the clock; the server pushes the clock, the meters and notices to the
//! clients that subscribed.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::events::{At, Cancelled, Op, Sent};
use crate::session::{ClockInfo, Session};
use niminal_lang::Problem;

/// The protocol version a client must say it speaks.
pub const PROTOCOL_VERSION: u64 = 5;

pub type ClientId = u64;

/// The release this server is part of (`0.0.1-alpha.10`), told to every client in the `hello` reply. Set by
/// the program that runs the server; a development build has none. An app of the same release checks it, to
/// notice a server left running from an older install.
static RELEASE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Say which release this server is, in its `hello` reply.
pub fn set_release(release: &str) {
    let _ = RELEASE.set(release.to_string());
}

/// The release this program is part of, if it was built as one.
pub fn release() -> Option<&'static str> {
    RELEASE.get().map(String::as_str)
}

pub(crate) const PARSE_ERROR: i64 = -32700;
pub(crate) const INVALID_REQUEST: i64 = -32600;
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;
pub(crate) const INVALID_PARAMS: i64 = -32602;
pub(crate) const INTERNAL_ERROR: i64 = -32603;
/// The client has not said hello, or speaks another version.
pub(crate) const PROTOCOL_ERROR: i64 = -32000;
/// The events sent were not accepted; `data.problems` says why.
pub(crate) const REJECTED: i64 = -32001;

/// What a client of the engine can subscribe to.
pub const ENGINE_TOPICS: [&str; 6] = [
    "clock",
    "meters",
    "notices",
    "transactions",
    "voices",
    "reports",
];

/// A request to the engine, as it is named on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum EngineRequest {
    #[serde(rename = "hello")]
    Hello {
        protocol: u64,
        #[serde(default)]
        client: Option<String>,
        #[serde(default)]
        token: Option<String>,
    },
    #[serde(rename = "subscribe")]
    Subscribe { topics: Vec<String> },
    #[serde(rename = "unsubscribe")]
    Unsubscribe { topics: Vec<String> },
    #[serde(rename = "events.send")]
    EventsSend { events: Vec<Op> },
    #[serde(rename = "events.cancel")]
    EventsCancel {
        #[serde(default)]
        tag: Option<String>,
        #[serde(default)]
        from: Option<At>,
        #[serde(default)]
        release: bool,
    },
    #[serde(rename = "engine.panic")]
    EnginePanic {},
    #[serde(rename = "panic")]
    Panic {},
    #[serde(rename = "status")]
    Status {},
    #[serde(rename = "clock")]
    Clock {},
}



/// What the engine answers.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineReply {
    Hello {
        sample_rate: f32,
        channels: usize,
        client_id: ClientId,
    },
    Subscribed(Vec<String>),
    Sent {
        sent: Sent,
        horizon: u64,
    },
    Cancelled(Cancelled),
    Panicked,
    Json(Value),
    Clock(ClockInfo),
}

/// A request the engine refused.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl EngineError {
    /// Read an error object from a reply across a wire.
    pub fn from_json(error: &Value) -> EngineError {
        EngineError {
            code: error["code"].as_i64().unwrap_or(INTERNAL_ERROR),
            message: error["message"]
                .as_str()
                .unwrap_or("the engine refused that")
                .to_string(),
            data: error.get("data").cloned(),
        }
    }

    pub(crate) fn new(code: i64, message: &str) -> EngineError {
        EngineError {
            code,
            message: message.to_string(),
            data: None,
        }
    }

    /// The events were refused, for these reasons.
    pub fn rejected(problems: &[Problem]) -> EngineError {
        let first = problems
            .first()
            .map_or("the code did not compile", |p| p.message.as_str());
        EngineError {
            code: REJECTED,
            message: first.to_string(),
            data: Some(
                json!({ "problems": problems.iter().map(problem_json).collect::<Vec<_>>() }),
            ),
        }
    }

    /// The problems, if the request was refused with some.
    pub fn problems(&self) -> Vec<String> {
        self.data
            .as_ref()
            .and_then(|d| d["problems"].as_array())
            .map(|ps| {
                ps.iter()
                    .filter_map(|p| p["message"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EngineError {}

pub fn problem_json(p: &Problem) -> Value {
    json!({
        "message": p.message,
        "help": p.help,
        "line": p.line,
        "column": p.column,
        "span": p.span.map(|s| json!({"start": s.start, "end": s.end})),
        "in_definition": p.in_definition,
        "event": p.event,
    })
}

impl EngineReply {
    /// Read the reply to `request` from the result a server sent, for a client across a wire.
    pub fn from_json(request: &EngineRequest, result: &Value) -> Result<EngineReply, String> {
        fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, String> {
            serde_json::from_value(value.clone()).map_err(|e| format!("invalid engine reply: {e}"))
        }
        Ok(match request {
            EngineRequest::Hello { .. } => {
                #[derive(Deserialize)]
                struct Hello {
                    client_id: ClientId,
                    sample_rate: f32,
                    channels: usize,
                }
                let reply: Hello = decode(result)?;
                EngineReply::Hello {
                    client_id: reply.client_id,
                    sample_rate: reply.sample_rate,
                    channels: reply.channels,
                }
            }
            EngineRequest::Subscribe { .. } | EngineRequest::Unsubscribe { .. } => {
                #[derive(Deserialize)]
                struct Subscribed {
                    topics: Vec<String>,
                }
                EngineReply::Subscribed(decode::<Subscribed>(result)?.topics)
            }
            EngineRequest::EventsSend { .. } => {
                #[derive(Deserialize)]
                struct SentReply {
                    #[serde(flatten)]
                    sent: Sent,
                    horizon: u64,
                }
                let reply: SentReply = decode(result)?;
                EngineReply::Sent {
                    sent: reply.sent,
                    horizon: reply.horizon,
                }
            }
            EngineRequest::EventsCancel { .. } => EngineReply::Cancelled(decode(result)?),
            EngineRequest::Panic {} | EngineRequest::EnginePanic {} => EngineReply::Panicked,
            EngineRequest::Status {} => EngineReply::Json(result.clone()),
            EngineRequest::Clock {} => EngineReply::Clock(decode(result)?),
        })
    }

    pub fn to_json(&self) -> Value {
        match self {
            EngineReply::Hello {
                sample_rate,
                channels,
                client_id,
            } => json!({
                "protocol": PROTOCOL_VERSION,
                "release": RELEASE.get(),
                "client_id": client_id,
                "sample_rate": sample_rate,
                "channels": channels,
                "topics": ENGINE_TOPICS,
                "capabilities": ["private_definitions", "routing", "transactions", "voices", "reports", "multichannel_buffers", "late_buffer_binding", "fx_modulation"],
                "limits": { "buffer_write_samples": 65536, "message_bytes": 4194304, "buffer_bytes": 268435456, "report_values": 64 },
            }),
            EngineReply::Subscribed(topics) => json!({ "topics": topics }),
            EngineReply::Sent { sent, horizon } => {
                let mut result = serde_json::to_value(sent).expect("serializable receipt");
                result["horizon"] = json!(horizon);
                result
            }
            EngineReply::Cancelled(c) => {
                serde_json::to_value(c).expect("serializable cancellation")
            }
            EngineReply::Panicked => json!({}),
            EngineReply::Json(v) => v.clone(),
            EngineReply::Clock(c) => clock_json(c),
        }
    }
}

pub fn clock_json(c: &ClockInfo) -> Value {
    serde_json::to_value(c).expect("serializable clock")
}

#[derive(Default)]
struct Client {
    local: bool,
    greeted: bool,
    topics: BTreeSet<String>,
}

pub struct EngineServer {
    session: Session,
    clients: BTreeMap<ClientId, Client>,
    next_client: ClientId,
    last_clock: Option<u64>,
    clock_every: u64,
    outbox: Vec<(ClientId, String)>,
    reports: crate::reports::Reports,
    /// Verbosity and where log lines go. A separate thread prints them, so a slow terminal never holds up the engine.
    trace: Option<(u8, std::sync::mpsc::Sender<String>)>,
}

impl EngineServer {
    pub fn new(session: Session) -> EngineServer {
        let clock_every = (session.sample_rate() / 10.0) as u64;
        EngineServer {
            session,
            clients: BTreeMap::new(),
            next_client: 1,
            last_clock: None,
            clock_every,
            outbox: Vec::new(),
            reports: Default::default(),
            trace: None,
        }
    }

    /// Send a line to `lines` for each thing that happens at or below `level`: 1 covers clients
    /// coming and going and requests that fail, 2 every request.
    pub fn set_trace(&mut self, level: u8, lines: std::sync::mpsc::Sender<String>) {
        self.trace = Some((level, lines));
    }

    fn trace(&self, level: u8, line: impl FnOnce() -> String) {
        if let Some((max, lines)) = &self.trace
            && level <= *max
        {
            let _ = lines.send(line());
        }
    }

    pub fn is_connected(&self, client: u64) -> bool {
        self.clients.contains_key(&client)
    }
    pub fn reset_session(&mut self, session: Session) {
        self.clients.clear();
        self.outbox.clear();
        self.reports = Default::default();
        self.session = session;
        self.last_clock = None;
        self.clock_every = (self.session.sample_rate() / 10.) as u64;
    }
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// For the audio side, which advances the clock.
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    pub fn connect(&mut self) -> ClientId {
        let id = self.next_client;
        self.next_client += 1;
        self.clients.insert(id, Client::default());
        self.trace(1, || format!("client {id} connected"));
        id
    }

    /// A client that collects its own notifications instead of using the network server's outbox.
    pub fn connect_local(&mut self) -> ClientId {
        let id = self.connect();
        self.clients.get_mut(&id).unwrap().local = true;
        id
    }

    pub fn disconnect(&mut self, client: ClientId) {
        self.trace(1, || format!("client {client} disconnected"));
        self.clients.remove(&client);
        self.outbox.retain(|(id, _)| *id != client);
        self.session.drop_client(client);
    }

    /// Handle one message from `client`; the replies to send back to it.
    pub fn handle(&mut self, client: ClientId, text: &str) -> Vec<String> {
        let request: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                return vec![error_reply(
                    Value::Null,
                    PARSE_ERROR,
                    &format!("parse error: {e}"),
                    None,
                )];
            }
        };
        let id = request.get("id").cloned();
        let reply_to = id.clone().unwrap_or(Value::Null);
        let bad = |why: &str| vec![error_reply(reply_to.clone(), INVALID_REQUEST, why, None)];
        if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return bad("`jsonrpc` must be \"2.0\"");
        }
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            return bad("a request needs a `method`");
        };
        let params = match request.get("params") {
            None | Some(Value::Null) => json!({}),
            Some(p) => p.clone(),
        };
        let started = std::time::Instant::now();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let parsed: EngineRequest = serde_json::from_value(
                json!({ "method": method, "params": params }),
            )
            .map_err(|e| {
                let text = e.to_string();
                if text.contains("unknown variant") {
                    EngineError::new(METHOD_NOT_FOUND, &format!("no method `{method}`"))
                } else {
                    EngineError::new(
                        INVALID_PARAMS,
                        &format!("bad params for `{method}`: {text}"),
                    )
                }
            })?;
            self.request(client, parsed)
        }))
        .unwrap_or_else(|_| Err(internal_error()));
        match &outcome {
            // the clock is polled several times a second, which would drown everything else
            Ok(_) if method == "clock" => {}
            Ok(_) => self.trace(2, || {
                format!(
                    "client {client} {method} ok in {:.1} ms",
                    started.elapsed().as_secs_f64() * 1e3
                )
            }),
            Err(e) => self.trace(1, || {
                format!("client {client} {method} failed: {}", e.message)
            }),
        }
        match (id, outcome) {
            (None, _) => Vec::new(), // a notification from the client gets no reply
            (Some(id), Ok(reply)) => {
                vec![json!({"jsonrpc": "2.0", "id": id, "result": reply.to_json()}).to_string()]
            }
            (Some(id), Err(e)) => vec![error_reply(id, e.code, &e.message, e.data)],
        }
    }

    /// Do what `client` asks.
    pub fn request(
        &mut self,
        client: ClientId,
        request: EngineRequest,
    ) -> Result<EngineReply, EngineError> {
        if let EngineRequest::Hello { protocol, .. } = &request {
            if *protocol != PROTOCOL_VERSION {
                return Err(EngineError::new(
                    PROTOCOL_ERROR,
                    &format!(
                        "this engine speaks protocol {PROTOCOL_VERSION}, the client said {protocol}; update whichever is older, so that they come from the same release"
                    ),
                ));
            }
            if let Some(c) = self.clients.get_mut(&client) {
                c.greeted = true;
            }
            self.trace(1, || {
                let name = match &request {
                    EngineRequest::Hello {
                        client: Some(name), ..
                    } => name.as_str(),
                    _ => "unnamed",
                };
                format!("client {client} said hello as {name}")
            });
            return Ok(EngineReply::Hello {
                client_id: client,
                sample_rate: self.session.sample_rate(),
                channels: self.session.channels(),
            });
        }
        if !self.clients.get(&client).is_some_and(|c| c.greeted) {
            return Err(EngineError::new(
                PROTOCOL_ERROR,
                "say hello first, with the protocol version",
            ));
        }
        self.session.sync_realtime();
        match request {
            EngineRequest::Hello { .. } => unreachable!("handled above"),
            EngineRequest::Subscribe { topics } => self.subscribe(client, &topics, true),
            EngineRequest::Unsubscribe { topics } => self.subscribe(client, &topics, false),
            EngineRequest::EventsSend { events } => {
                for event in &events {
                    let is_sample =
                        |v: &niminal_score::Value| matches!(v, niminal_score::Value::Sample(_));
                    let has_sample = match event {
                        Op::Note { args, .. } | Op::Set { args, .. } => {
                            args.values().any(is_sample)
                        }
                        Op::Array { values, .. } => values.iter().any(is_sample),
                        _ => false,
                    };
                    if has_sample {
                        return Err(EngineError::new(
                            INVALID_PARAMS,
                            "use instrument-local numeric sample indices; resolve compiler sample IDs in the client",
                        ));
                    }
                    let value = serde_json::to_value(event).expect("serializable event");
                    if contains_reserved_namespace(&value) {
                        return Err(EngineError::new(
                            INVALID_PARAMS,
                            "internal namespace names are reserved",
                        ));
                    }
                }
                let sent = self.session.send_events(client, events).map_err(|mut p| {
                    for problem in &mut p {
                        problem.message =
                            problem.message.replace(&format!("__client_{client}::"), "");
                        if let Some(name) = &mut problem.in_definition {
                            *name = name.replace(&format!("__client_{client}::"), "");
                        }
                    }
                    EngineError::rejected(&p)
                })?;
                self.reports.register(&self.session);
                Ok(EngineReply::Sent {
                    sent,
                    horizon: self.session.prepared_until(),
                })
            }
            EngineRequest::EventsCancel { tag, from, release } => {
                let done = self.session.cancel_events(
                    client,
                    tag.as_deref(),
                    from.unwrap_or(At::Now),
                    release,
                );
                Ok(EngineReply::Cancelled(done))
            }
            EngineRequest::EnginePanic {} => {
                self.session.panic();
                Ok(EngineReply::Panicked)
            }
            EngineRequest::Panic {} => {
                self.session.panic_client(client);
                Ok(EngineReply::Panicked)
            }
            EngineRequest::Status {} => {
                let mut status = crate::status::client_status_json(&self.session, client);
                status["active_voices"] = self.session.active_voice_status(client);
                status["client_id"] = json!(client);
                status["transactions"] = self.session.transaction_status(client);
                Ok(EngineReply::Json(status))
            }
            EngineRequest::Clock {} => Ok(EngineReply::Clock(self.session.clock_info())),
        }
    }

    fn subscribe(
        &mut self,
        client: ClientId,
        topics: &[String],
        on: bool,
    ) -> Result<EngineReply, EngineError> {
        for t in topics {
            if !ENGINE_TOPICS.contains(&t.as_str()) {
                return Err(EngineError::new(
                    INVALID_PARAMS,
                    &format!(
                        "no topic `{t}`; the topics are {}",
                        ENGINE_TOPICS.join(", ")
                    ),
                ));
            }
        }
        let state = self.clients.get_mut(&client).expect("a connected client");
        for t in topics {
            if on {
                state.topics.insert(t.clone());
            } else {
                state.topics.remove(t);
            }
        }
        Ok(EngineReply::Subscribed(
            state.topics.iter().cloned().collect(),
        ))
    }

    fn has_subscribers(&self, topic: &str) -> bool {
        self.clients
            .values()
            .any(|c| c.greeted && c.topics.contains(topic))
    }

    fn send_to_subscribers(&mut self, topic: &str, method: &str, params: Value) {
        let text = json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string();
        for (id, client) in &self.clients {
            if client.greeted && client.topics.contains(topic) {
                self.outbox.push((*id, text.clone()));
            }
        }
    }

    /// Everything the server has to say that nobody asked for, since this was last called, as
    /// `(client, message)`. Call it regularly while the clock advances: it reports notices and, about ten
    /// times a second, the clock and meters.
    pub fn notifications(&mut self) -> Vec<(ClientId, String)> {
        self.collect_notifications();
        self.outbox
            .extract_if(.., |(id, _)| self.clients.get(id).is_none_or(|c| !c.local))
            .collect()
    }

    /// Collect only this in-process client's messages, preserving those for network clients.
    pub fn notifications_for(&mut self, client: ClientId) -> Vec<(ClientId, String)> {
        self.collect_notifications();
        self.outbox
            .extract_if(.., |(id, _)| *id == client)
            .collect()
    }

    fn send_to_client_subscriber(
        &mut self,
        client: ClientId,
        topic: &str,
        method: &str,
        params: Value,
    ) {
        if self
            .clients
            .get(&client)
            .is_some_and(|c| c.greeted && c.topics.contains(topic))
        {
            self.outbox.push((
                client,
                json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string(),
            ));
        }
    }

    fn collect_notifications(&mut self) {
        self.session.sync_realtime();
        for (client, params) in self.reports.poll(&self.session) {
            self.send_to_client_subscriber(client, "reports", "report", params);
        }
        let clients = &self.clients;
        let listening = |client| {
            clients
                .get(&client)
                .is_some_and(|c| c.greeted && c.topics.contains("voices"))
        };
        for (client, params) in self.session.take_voice_updates(listening) {
            self.send_to_client_subscriber(client, "voices", "voice", params);
        }
        self.session
            .voice_sequence
            .retain(|client, _| self.clients.contains_key(client));
        for (client, params) in self.session.take_transaction_updates() {
            self.send_to_client_subscriber(client, "transactions", "transaction", params);
        }
        for notice in self.session.take_notices() {
            for (&client, state) in &self.clients {
                let prefix = format!("__client_{client}::");
                if state.topics.contains("notices")
                    && (!notice.contains("__client_") || notice.contains(&prefix))
                {
                    self.outbox.push((client,json!({"jsonrpc":"2.0","method":"notice","params":{"message":notice.replace(&prefix,"")}}).to_string()));
                }
            }
        }
        let silenced = self.session.take_silenced();
        if silenced > 0 {
            self.send_to_subscribers(
                "notices",
                "notice",
                json!({ "message": format!("{silenced} voice(s) produced NaN or infinity and were silenced") }),
            );
        }
        let now = self.session.clock();
        if self.last_clock.is_none_or(|t| now >= t + self.clock_every) {
            self.last_clock = Some(now);
            if self.has_subscribers("clock") {
                let clock = clock_json(&self.session.clock_info());
                self.send_to_subscribers("clock", "clock", clock);
            }
            // reading the meters resets them, so only do it for someone listening
            if self.has_subscribers("meters") {
                let peaks = self.session.take_peaks();
                let routing = self.session.routing_meters(true);
                for (&client, state) in &self.clients {
                    if state.topics.contains("meters") {
                        let own: Vec<_> = routing
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|r| r["client"].as_u64() == Some(client))
                            .cloned()
                            .collect();
                        self.outbox.push((client,json!({"jsonrpc":"2.0","method":"meters","params":{"peaks":peaks,"routing":own}}).to_string()));
                    }
                }
            }
        }
    }
}

fn internal_error() -> EngineError {
    EngineError::new(
        INTERNAL_ERROR,
        "niminal hit an internal error handling that; the engine carries on",
    )
}

pub(crate) fn error_reply(id: Value, code: i64, message: &str, data: Option<Value>) -> String {
    let mut error = json!({ "code": code, "message": message });
    if let Some(d) = data {
        error["data"] = d;
    }
    json!({"jsonrpc": "2.0", "id": id, "error": error}).to_string()
}

fn contains_reserved_namespace(v: &Value) -> bool {
    match v {
        Value::String(s) => s.contains("__client_"),
        Value::Array(a) => a.iter().any(contains_reserved_namespace),
        Value::Object(o) => o.values().any(contains_reserved_namespace),
        _ => false,
    }
}
