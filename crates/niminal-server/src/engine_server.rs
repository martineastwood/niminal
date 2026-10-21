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
pub const ENGINE_TOPICS: [&str; 8] = [
    "clock",
    "meters",
    "notices",
    "transactions",
    "scopes",
    "voices",
    "reports",
    "jobs",
];

/// A request to the engine, as it is named on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum EngineRequest {
    #[serde(rename = "audio.devices")]
    AudioDevices {},
    #[serde(rename = "audio.status")]
    AudioStatus {},
    #[serde(rename = "audio.configure")]
    AudioConfigure {
        settings: crate::AudioSettings,
        #[serde(default)]
        restart: bool,
    },
    #[serde(rename = "capture.start")]
    CaptureStart {
        output: crate::RenderOutput,
        #[serde(default = "now")]
        at: At,
    },
    #[serde(rename = "capture.stop")]
    CaptureStop {
        id: u64,
        #[serde(default = "now")]
        at: At,
    },
    #[serde(rename = "render.start")]
    RenderStart {
        #[serde(flatten)]
        spec: crate::RenderSpec,
    },
    #[serde(rename = "jobs.status")]
    JobsStatus { id: u64 },
    #[serde(rename = "jobs.cancel")]
    JobsCancel { id: u64 },
    #[serde(rename = "files.upload")]
    FilesUpload {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        offset: u64,
        #[serde(default)]
        data: String,
        #[serde(default)]
        finish: bool,
    },
    #[serde(rename = "files.read")]
    FilesRead {
        id: String,
        #[serde(default)]
        offset: u64,
        count: usize,
    },
    #[serde(rename = "files.remove")]
    FilesRemove { id: String },
    #[serde(rename = "scopes.create")]
    ScopesCreate {
        id: String,
        endpoint: crate::Endpoint,
        #[serde(default = "scope_frames")]
        frames: usize,
        #[serde(default = "scope_rate")]
        rate: f64,
    },
    #[serde(rename = "scopes.remove")]
    ScopesRemove { id: String },
    #[serde(rename = "buffers.budget")]
    BuffersBudget {
        #[serde(default)]
        bytes: Option<usize>,
    },
    #[serde(rename = "buffers.list")]
    BuffersList {},
    #[serde(rename = "buffers.read")]
    BuffersRead {
        name: String,
        #[serde(default)]
        offset: usize,
        count: usize,
    },
    #[serde(rename = "buffers.alloc")]
    BuffersAlloc {
        name: String,
        frames: usize,
        #[serde(default = "first_channel")]
        channels: usize,
    },
    #[serde(rename = "buffers.free")]
    BuffersFree { name: String },
    #[serde(rename = "buffers.load")]
    BuffersLoad {
        name: String,
        path: String,
        #[serde(default)]
        channel: usize,
    },
    #[serde(rename = "buffers.save")]
    BuffersSave { name: String, path: String },
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

fn now() -> At {
    At::Now
}
fn scope_frames() -> usize {
    1024
}
fn scope_rate() -> f64 {
    20.0
}

fn first_channel() -> usize {
    1
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
    Status(Value),
    Buffers(Value),
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
        let number = |key: &str| result[key].as_u64().unwrap_or(0);
        Ok(match request {
            EngineRequest::Hello { .. } => EngineReply::Hello {
                client_id: number("client_id"),
                sample_rate: result["sample_rate"].as_f64().unwrap_or(48_000.0) as f32,
                channels: number("channels") as usize,
            },
            EngineRequest::Subscribe { .. } | EngineRequest::Unsubscribe { .. } => {
                EngineReply::Subscribed(
                    result["topics"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|t| t.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                )
            }
            EngineRequest::EventsSend { .. } => EngineReply::Sent {
                sent: Sent {
                    notes: serde_json::from_value(result["notes"].clone()).unwrap_or_default(),
                    transactions: serde_json::from_value(result["transactions"].clone())
                        .unwrap_or_default(),
                    accepted: number("accepted") as usize,
                    late: number("late") as usize,
                    first: result["first"].as_u64(),
                },
                horizon: number("horizon"),
            },
            EngineRequest::EventsCancel { .. } => EngineReply::Cancelled(Cancelled {
                cancelled: number("cancelled") as usize,
                released: number("released") as usize,
            }),
            EngineRequest::Panic {} | EngineRequest::EnginePanic {} => EngineReply::Panicked,
            EngineRequest::BuffersList {}
            | EngineRequest::BuffersRead { .. }
            | EngineRequest::BuffersAlloc { .. }
            | EngineRequest::BuffersLoad { .. }
            | EngineRequest::BuffersSave { .. }
            | EngineRequest::BuffersFree { .. }
            | EngineRequest::BuffersBudget { .. } => EngineReply::Buffers(result.clone()),
            EngineRequest::Status {} => EngineReply::Status(result.clone()),
            EngineRequest::AudioDevices {}
            | EngineRequest::AudioStatus {}
            | EngineRequest::AudioConfigure { .. }
            | EngineRequest::CaptureStart { .. }
            | EngineRequest::CaptureStop { .. }
            | EngineRequest::RenderStart { .. }
            | EngineRequest::JobsStatus { .. }
            | EngineRequest::JobsCancel { .. }
            | EngineRequest::FilesUpload { .. }
            | EngineRequest::FilesRead { .. }
            | EngineRequest::FilesRemove { .. }
            | EngineRequest::ScopesCreate { .. }
            | EngineRequest::ScopesRemove { .. } => EngineReply::Status(result.clone()),
            EngineRequest::Clock {} => EngineReply::Clock(ClockInfo {
                sample: number("sample"),
                sample_rate: result["sample_rate"].as_f64().unwrap_or(48_000.0) as f32,
                prepared_until: number("prepared_until"),
                lookahead: number("lookahead"),
                output_latency: result["output_latency"].as_f64().unwrap_or(0.0),
                wall_micros: result["wall_micros"].as_u64(),
                voices: number("voices") as usize,
            }),
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
                "client_id": client_id,
                "sample_rate": sample_rate,
                "channels": channels,
                "topics": ENGINE_TOPICS,
                "capabilities": ["private_definitions", "routing", "transactions", "voices", "reports", "scopes", "multichannel_buffers", "late_buffer_binding", "fx_modulation", "file_transfer", "offline_render", "capture", "file_players", "audio_management"],
                "limits": { "buffer_write_samples": 65536, "message_bytes": 4194304, "buffer_bytes":268435456,"file_chunk_bytes":262144,"report_values":64,"scope_frames":4096,"scopes_per_client":8 },
            }),
            EngineReply::Subscribed(topics) => json!({ "topics": topics }),
            EngineReply::Sent { sent, horizon } => json!({
                "notes": sent.notes,
                "transactions": sent.transactions,
                "accepted": sent.accepted,
                "late": sent.late,
                "first": sent.first,
                "horizon": horizon,
            }),
            EngineReply::Cancelled(c) => {
                json!({ "cancelled": c.cancelled, "released": c.released })
            }
            EngineReply::Panicked => json!({}),
            EngineReply::Status(v) | EngineReply::Buffers(v) => v.clone(),
            EngineReply::Clock(c) => clock_json(c),
        }
    }
}

pub fn clock_json(c: &ClockInfo) -> Value {
    json!({
        "sample": c.sample,
        "sample_rate": c.sample_rate, "prepared_until": c.prepared_until, "lookahead": c.lookahead,
        "output_latency": c.output_latency, "wall_micros": c.wall_micros, "voices": c.voices,
    })
}

#[derive(Default)]
struct Client {
    local: bool,
    greeted: bool,
    topics: BTreeSet<String>,
}

pub struct EngineServer {
    audio: crate::AudioControl,
    session: Session,
    clients: BTreeMap<ClientId, Client>,
    next_client: ClientId,
    last_clock: Option<u64>,
    clock_every: u64,
    outbox: Vec<(ClientId, String)>,
    scopes: crate::scopes::Scopes,
    reports: crate::reports::Reports,
    files: crate::files::Files,
    jobs: crate::jobs::Jobs,
}

impl EngineServer {
    pub fn new(session: Session) -> EngineServer {
        let clock_every = (session.sample_rate() / 10.0) as u64;
        EngineServer {
            audio: Default::default(),
            session,
            clients: BTreeMap::new(),
            next_client: 1,
            last_clock: None,
            clock_every,
            outbox: Vec::new(),
            scopes: Default::default(),
            reports: Default::default(),
            files: Default::default(),
            jobs: Default::default(),
        }
    }

    pub fn set_audio_control(&mut self, audio: crate::AudioControl) {
        self.audio = audio;
    }
    pub fn is_connected(&self, client: u64) -> bool {
        self.clients.contains_key(&client)
    }
    pub fn reset_session(&mut self, session: Session) {
        let clients: Vec<_> = self.clients.keys().copied().collect();
        for client in clients {
            self.jobs.disconnect(client);
            self.files.disconnect(client);
            self.scopes.disconnect(client);
        }
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
        id
    }

    /// A client that collects its own notifications instead of using the network server's outbox.
    pub fn connect_local(&mut self) -> ClientId {
        let id = self.connect();
        self.clients.get_mut(&id).unwrap().local = true;
        id
    }

    pub fn disconnect(&mut self, client: ClientId) {
        self.clients.remove(&client);
        self.outbox.retain(|(id, _)| *id != client);
        self.jobs.disconnect(client);
        self.files.disconnect(client);
        self.scopes.disconnect(client);
        self.session.drop_client(client);
    }

    /// Handle one message from `client`; the replies to send back to it.
    pub fn handle(&mut self, client: ClientId, text: &str) -> Vec<String> {
        self.session.sync_realtime();
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
                        "this engine speaks protocol {PROTOCOL_VERSION}, the client said {protocol}"
                    ),
                ));
            }
            if let Some(c) = self.clients.get_mut(&client) {
                c.greeted = true;
            }
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
            EngineRequest::EventsSend { mut events } => {
                for event in &events {
                    let values = match event {
                        Op::Note { args, .. } | Op::Set { args, .. } => {
                            args.values().collect::<Vec<_>>()
                        }
                        Op::Array { values, .. } => values.iter().collect(),
                        _ => Vec::new(),
                    };
                    if values
                        .iter()
                        .any(|v| matches!(v, niminal_score::Value::Sample(_)))
                    {
                        return Err(EngineError::new(
                            INVALID_PARAMS,
                            "use instrument-local numeric sample indices; resolve compiler sample IDs in the client",
                        ));
                    }
                    let value = serde_json::to_value(event).expect("serializable event");
                    fn reserved(v: &Value) -> bool {
                        match v {
                            Value::String(s) => s.contains("__client_"),
                            Value::Array(a) => a.iter().any(reserved),
                            Value::Object(o) => o.values().any(reserved),
                            _ => false,
                        }
                    }
                    if reserved(&value) {
                        return Err(EngineError::new(
                            INVALID_PARAMS,
                            "internal namespace names are reserved",
                        ));
                    }
                }
                for event in &mut events {
                    if let Op::Routing { edits, .. } | Op::Transaction { edits, .. } = event {
                        for edit in edits {
                            if let crate::RoutingEdit::Player { player, .. } = edit
                                && player.path.starts_with("file:")
                            {
                                player.path = self
                                    .files
                                    .path(client, &player.path)
                                    .map_err(|e| EngineError::new(INVALID_PARAMS, &e))?
                                    .to_string_lossy()
                                    .into_owned();
                            }
                        }
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
            request @ (EngineRequest::BuffersList {}
            | EngineRequest::BuffersRead { .. }
            | EngineRequest::BuffersAlloc { .. }
            | EngineRequest::BuffersLoad { .. }
            | EngineRequest::BuffersSave { .. }
            | EngineRequest::BuffersFree { .. }
            | EngineRequest::BuffersBudget { .. }) => {
                let mut request = request;
                match &mut request {
                    EngineRequest::BuffersLoad { path, .. } if path.starts_with("file:") => {
                        *path = self
                            .files
                            .path(client, path)
                            .map_err(|e| EngineError::new(INVALID_PARAMS, &e))?
                            .to_string_lossy()
                            .into_owned();
                    }
                    _ => {}
                }
                crate::resources::request(&mut self.session, client, request)
                    .map(EngineReply::Buffers)
            }
            EngineRequest::Status {} => {
                let mut status = crate::status::client_status_json(&self.session, client);
                status["active_voices"] = self.session.active_voice_status(client);
                status["client_id"] = json!(client);
                status["transactions"] = self.session.transaction_status(client);
                Ok(EngineReply::Status(status))
            }
            EngineRequest::AudioDevices {} => Ok(EngineReply::Status(self.audio.devices())),
            EngineRequest::AudioStatus {} => Ok(EngineReply::Status(self.audio.status())),
            EngineRequest::AudioConfigure { settings, restart } => self
                .audio
                .configure(settings, restart)
                .map(EngineReply::Status)
                .map_err(|e| EngineError::new(INVALID_PARAMS, &e)),
            EngineRequest::CaptureStart { output, at } => self
                .jobs
                .capture(&self.session, client, output, at)
                .map(EngineReply::Status)
                .map_err(|e| EngineError::new(INVALID_PARAMS, &e)),
            EngineRequest::CaptureStop { id, at } => self
                .jobs
                .stop_capture(
                    client,
                    id,
                    match at {
                        At::Now => self.session.clock(),
                        At::Sample(n) => n,
                    },
                )
                .map(EngineReply::Status)
                .map_err(|e| EngineError::new(INVALID_PARAMS, &e)),
            EngineRequest::RenderStart { spec } => self
                .jobs
                .render(client, spec, &self.files)
                .map(EngineReply::Status)
                .map_err(|e| EngineError::new(INVALID_PARAMS, &e)),
            EngineRequest::JobsStatus { id } => self
                .jobs
                .status(client, id, &mut self.files)
                .map(EngineReply::Status)
                .map_err(|e| EngineError::new(INVALID_PARAMS, &e)),
            EngineRequest::JobsCancel { id } => self
                .jobs
                .cancel(client, id)
                .map(EngineReply::Status)
                .map_err(|e| EngineError::new(INVALID_PARAMS, &e)),
            EngineRequest::FilesUpload {
                id,
                offset,
                data,
                finish,
            } => self
                .files
                .upload(client, id, offset, &data, finish)
                .map(EngineReply::Status)
                .map_err(|e| EngineError::new(INVALID_PARAMS, &e)),
            EngineRequest::FilesRead { id, offset, count } => self
                .files
                .read(client, &id, offset, count)
                .map(EngineReply::Status)
                .map_err(|e| EngineError::new(INVALID_PARAMS, &e)),
            EngineRequest::FilesRemove { id } => self
                .files
                .remove(client, &id)
                .map(|_| EngineReply::Status(json!({})))
                .map_err(|e| EngineError::new(INVALID_PARAMS, &e)),
            EngineRequest::ScopesCreate {
                id,
                endpoint,
                frames,
                rate,
            } => self
                .scopes
                .create(&self.session, client, id, endpoint, frames, rate)
                .map(EngineReply::Status)
                .map_err(|e| EngineError::new(INVALID_PARAMS, &e)),
            EngineRequest::ScopesRemove { id } => {
                self.scopes.remove(client, &id);
                Ok(EngineReply::Status(json!({})))
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

    fn collect_notifications(&mut self) {
        self.session.sync_realtime();
        for (client, params) in self.jobs.poll(&mut self.files) {
            if self
                .clients
                .get(&client)
                .is_some_and(|c| c.topics.contains("jobs"))
            {
                self.outbox.push((
                    client,
                    json!({"jsonrpc":"2.0","method":"job","params":params}).to_string(),
                ));
            }
        }
        for (client, params) in self.reports.poll(&self.session) {
            if self
                .clients
                .get(&client)
                .is_some_and(|c| c.topics.contains("reports"))
            {
                self.outbox.push((
                    client,
                    json!({"jsonrpc":"2.0","method":"report","params":params}).to_string(),
                ));
            }
        }
        for (client, params) in self.session.take_voice_updates() {
            if self
                .clients
                .get(&client)
                .is_some_and(|c| c.topics.contains("voices"))
            {
                self.outbox.push((
                    client,
                    json!({"jsonrpc":"2.0","method":"voice","params":params}).to_string(),
                ));
            }
        }
        self.session
            .voice_sequence
            .retain(|client, _| self.clients.contains_key(client));
        for (client, params) in self.scopes.poll(&self.session) {
            if self
                .clients
                .get(&client)
                .is_some_and(|c| c.topics.contains("scopes"))
            {
                self.outbox.push((
                    client,
                    json!({"jsonrpc":"2.0","method":"scope","params":params}).to_string(),
                ));
            }
        }
        for (id, update) in self.session.take_transaction_updates() {
            if self
                .clients
                .get(&id)
                .is_some_and(|c| c.greeted && c.topics.contains("transactions"))
            {
                self.outbox.push((
                    id,
                    json!({"jsonrpc":"2.0","method":"transaction","params":update}).to_string(),
                ));
            }
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
