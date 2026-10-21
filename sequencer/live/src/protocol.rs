//! JSON-RPC 2.0 over whatever carries the messages (a WebSocket, in practice).
//!
//! A [`Server`] owns a [`Session`] and speaks the protocol to any number of
//! clients: requests come in as text and replies go out as text, and the
//! server pushes notifications (changes landing, the transport, what is
//! waiting) to the clients that subscribed.

use std::collections::{BTreeMap, BTreeSet};

use niminal_compose::{ClipDef, SceneDef};
use niminal_server::{ClientId, PROTOCOL_VERSION, Problem, problem_json};
use serde_json::{Value, json};

use crate::live::{
    Accepted, Begin, CancelDone, CancelJob, EvalDone, EvalJob, Landed, Live, ParsedEval,
    PendingInfo, Transport,
};

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
/// The client has not said hello, or speaks another version.
const PROTOCOL_ERROR: i64 = -32000;
/// The code sent did not compile; `data.problems` says why.
const REJECTED: i64 = -32001;

/// What a client can subscribe to.
pub const TOPICS: [&str; 6] = [
    "landed",
    "pending",
    "transport",
    "meters",
    "notices",
    "learned",
];

#[derive(Default)]
struct Client {
    greeted: bool,
    topics: BTreeSet<String>,
}

pub struct LiveServer {
    live: Live,
    metering: bool,
    clients: BTreeMap<ClientId, Client>,
    next_client: ClientId,
    /// The waiting changes last announced, to tell when they change.
    announced: Vec<(u64, u64)>,
    last_transport: Option<u64>,
    transport_every: u64,
    outbox: Vec<(ClientId, String)>,
}

impl LiveServer {
    pub fn new(live: Live) -> LiveServer {
        let transport_every = (live.sample_rate() / 10.0) as u64;
        LiveServer {
            live,
            metering: false,
            clients: BTreeMap::new(),
            next_client: 1,
            announced: Vec::new(),
            last_transport: None,
            transport_every,
            outbox: Vec::new(),
        }
    }

    pub fn live(&self) -> &Live {
        &self.live
    }

    /// For the thread that keeps the sequencer working ahead of the engine's clock.
    pub fn live_mut(&mut self) -> &mut Live {
        &mut self.live
    }

    pub fn connect(&mut self) -> ClientId {
        let id = self.next_client;
        self.next_client += 1;
        self.clients.insert(id, Client::default());
        id
    }

    pub fn disconnect(&mut self, client: ClientId) {
        self.clients.remove(&client);
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
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

        // A bug in handling one request must not take the whole session down.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.dispatch(client, method, &params)
        }))
        .unwrap_or_else(|_| Err(internal_error()));
        self.respond(id, outcome)
    }

    fn respond(&mut self, id: Option<Value>, outcome: Result<Value, RpcError>) -> Vec<String> {
        self.queue_changes();
        match (id, outcome) {
            (None, _) => Vec::new(), // a notification from the client gets no reply
            (Some(id), Ok(result)) => {
                vec![json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()]
            }
            (Some(id), Err(e)) => vec![error_reply(id, e.code, &e.message, e.data)],
        }
    }

    /// Like [`Server::handle`], but an `eval` comes back as a ticket to compile
    /// away from the server (see [`CompileTicket::compile`]) and hand to
    /// [`Server::finish`]. A server does that, so that compiling never holds
    /// up the audio thread. Everything else is answered at once.
    pub fn begin(&mut self, client: ClientId, text: &str) -> Request {
        self.begin_prepared(client, text, Self::prepare_eval(text))
    }

    /// Parse potentially large source before taking the render worker's lock.
    pub fn prepare_eval(text: &str) -> Option<ParsedEval> {
        let request: Value = serde_json::from_str(text).ok()?;
        let method = request.get("method").and_then(Value::as_str)?;
        let params = &request["params"];
        let quantize = params.get("quantize").and_then(Value::as_str);
        match method {
            "eval" => {
                let dir = params["dir"].as_str().unwrap_or(".");
                Some(ParsedEval::from_dir(
                    params["source"].as_str()?,
                    std::path::Path::new(dir),
                    quantize,
                ))
            }
            "hush" => Some(ParsedEval::new("hush", Some("now"))),
            "clip.define" | "scene.define" | "launch" | "stop" | "mute" | "unmute" | "solo"
            | "unsolo" | "edit" => {
                let source = api_source(method, params).ok()?;
                Some(ParsedEval::new(&source, quantize.or(Some("now"))))
            }
            _ => None,
        }
    }

    pub fn begin_prepared(
        &mut self,
        client: ClientId,
        text: &str,
        prepared: Option<ParsedEval>,
    ) -> Request {
        let parsed: Option<Value> = serde_json::from_str(text).ok();
        if let Some(request) = parsed.as_ref().filter(|r| {
            r.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
                && self.clients.get(&client).is_some_and(|c| c.greeted)
        }) {
            match request.get("method").and_then(Value::as_str) {
                Some("hush") => {
                    let mut request = request.clone();
                    request["method"] = json!("eval");
                    request["params"] = json!({ "source": "hush", "quantize": "now" });
                    return self.begin_prepared(client, &request.to_string(), prepared);
                }
                Some("cancel") => {
                    if let Ok(id) = optional_u64(&request["params"], "id") {
                        return Request::Compile(Box::new(CompileTicket {
                            id: request.get("id").cloned(),
                            job: Job::Cancel(self.live.begin_cancel(id)),
                        }));
                    }
                }
                _ => {}
            }
        }
        let method = parsed
            .as_ref()
            .and_then(|r| r.get("method").and_then(Value::as_str));
        let is_api = matches!(
            method,
            Some(
                "eval"
                    | "clip.define"
                    | "scene.define"
                    | "launch"
                    | "stop"
                    | "mute"
                    | "unmute"
                    | "solo"
                    | "unsolo"
                    | "edit"
            )
        );
        let eval = parsed.as_ref().filter(|r| {
            r.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
                && is_api
                && (method == Some("eval")
                    && r["params"].get("source").is_some_and(Value::is_string)
                    || method != Some("eval") && prepared.is_some())
                && self.clients.get(&client).is_some_and(|c| c.greeted)
        });
        let Some(request) = eval else {
            return Request::Replies(self.handle(client, text));
        };

        let id = request.get("id").cloned();
        let params = &request["params"];
        let source = params["source"].as_str().unwrap_or_default();
        let quantize = params.get("quantize").and_then(Value::as_str);
        if let Some(dir) = params.get("dir").and_then(Value::as_str) {
            self.live.set_sample_dir(std::path::Path::new(dir));
        }
        let begun = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match prepared {
            Some(parsed) => self.live.begin_parsed(parsed),
            None => self.live.begin_eval(source, quantize),
        }));
        match begun {
            Ok(Begin::Job(job)) => Request::Compile(Box::new(CompileTicket {
                id,
                job: Job::Eval(job),
            })),
            Ok(Begin::Done(result)) => {
                let outcome = self.eval_outcome(result);
                Request::Replies(self.respond(id, outcome))
            }
            Err(_) => Request::Replies(self.respond(id, Err(internal_error()))),
        }
    }

    /// A compilation made stale by another client or a landed change must be
    /// retried without holding the render worker's control lock.
    pub fn retry(&mut self, ticket: &CompiledTicket) -> Option<CompileTicket> {
        let job = match ticket.done.as_ref()? {
            Done::Eval(done) => Job::Eval(self.live.retry_eval(done)?),
            Done::Cancel(done) => Job::Cancel(self.live.retry_cancel(done)?),
        };
        Some(CompileTicket {
            id: ticket.id.clone(),
            job,
        })
    }

    /// Reply to work compiled away from the session. The server checks `retry`
    /// under the same lock before calling this, so no compilation is needed here.
    pub fn finish(&mut self, ticket: CompiledTicket) -> Vec<String> {
        let CompiledTicket { id, done } = ticket;
        let outcome = match done {
            None => Err(internal_error()),
            Some(done) => {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match done {
                    Done::Eval(done) => {
                        let result = self.live.finish_eval(done);
                        self.eval_outcome(result)
                    }
                    Done::Cancel(done) => Ok(json!({ "cancelled": self.live.finish_cancel(done) })),
                })) {
                    Ok(result) => result,
                    Err(_) => Err(internal_error()),
                }
            }
        };
        self.respond(id, outcome)
    }

    fn eval_outcome(&self, result: Result<Accepted, Vec<Problem>>) -> Result<Value, RpcError> {
        match result {
            Ok(a) => Ok(accepted_json(&self.live, &a)),
            Err(problems) => Err(rejected(&problems)),
        }
    }

    fn dispatch(
        &mut self,
        client: ClientId,
        method: &str,
        params: &Value,
    ) -> Result<Value, RpcError> {
        if method == "hello" {
            return self.hello(client, params);
        }
        if !self.clients.get(&client).is_some_and(|c| c.greeted) {
            return Err(RpcError::new(
                PROTOCOL_ERROR,
                "say hello first, with the protocol version",
            ));
        }
        match method {
            "eval" => self.eval(params),
            "clip.define" | "scene.define" | "launch" | "stop" | "mute" | "unmute" | "solo"
            | "unsolo" | "edit" => self.api_eval(method, params),
            "cancel" => {
                let id = optional_u64(params, "id")?;
                Ok(json!({ "cancelled": self.live.cancel(id) }))
            }
            "hush" => match self.live.hush() {
                Ok(a) => Ok(accepted_json(&self.live, &a)),
                Err(problems) => Err(rejected(&problems)),
            },
            "panic" => {
                self.live.panic();
                Ok(json!({}))
            }
            "subscribe" | "unsubscribe" => self.subscribe(client, params, method == "subscribe"),
            "status" => Ok(status_json(&mut self.live)),
            other => Err(RpcError::new(
                METHOD_NOT_FOUND,
                &format!("no method `{other}`"),
            )),
        }
    }

    /// Typed performance calls: build canonical source and evaluate it.
    fn api_eval(&mut self, method: &str, params: &Value) -> Result<Value, RpcError> {
        let source = api_source(method, params)?;
        let quantize = params
            .get("quantize")
            .and_then(Value::as_str)
            .or(Some("now"));
        let result = self.live.eval(&source, quantize);
        self.eval_outcome(result)
    }

    fn hello(&mut self, client: ClientId, params: &Value) -> Result<Value, RpcError> {
        let version = params.get("protocol").and_then(Value::as_u64);
        if version != Some(PROTOCOL_VERSION) {
            return Err(RpcError::new(
                PROTOCOL_ERROR,
                &format!(
                    "this server speaks protocol {PROTOCOL_VERSION}, the client said {version:?}"
                ),
            ));
        }
        if let Some(c) = self.clients.get_mut(&client) {
            c.greeted = true;
        }
        Ok(json!({
            "protocol": PROTOCOL_VERSION,
            "sample_rate": self.live.sample_rate(),
            "channels": self.live.channels(),
            "topics": TOPICS,
        }))
    }

    fn eval(&mut self, params: &Value) -> Result<Value, RpcError> {
        let source = params
            .get("source")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`eval` needs a string `source`"))?;
        let quantize = params.get("quantize").and_then(Value::as_str);
        let dir = params
            .get("dir")
            .and_then(Value::as_str)
            .map(std::path::Path::new);
        if let Some(dir) = dir {
            self.live.set_sample_dir(dir);
        }
        let result = self.live.eval_parsed(ParsedEval::from_dir(
            source,
            dir.unwrap_or(std::path::Path::new(".")),
            quantize,
        ));
        self.eval_outcome(result)
    }

    fn subscribe(&mut self, client: ClientId, params: &Value, on: bool) -> Result<Value, RpcError> {
        let topics: Vec<String> = params
            .get("topics")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect()
            })
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`topics` should be a list of names"))?;
        for t in &topics {
            if !TOPICS.contains(&t.as_str()) {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    &format!("no topic `{t}`; the topics are {}", TOPICS.join(", ")),
                ));
            }
        }
        let state = self.clients.get_mut(&client).expect("a connected client");
        for t in topics {
            if on {
                state.topics.insert(t);
            } else {
                state.topics.remove(&t);
            }
        }
        Ok(json!({ "topics": state.topics }))
    }

    // ---- notifications -------------------------------------------------------------

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

    /// Tell subscribers when the set of waiting changes differs from last time.
    fn queue_changes(&mut self) {
        let pending = self.live.pending();
        let now: Vec<(u64, u64)> = pending.iter().map(|p| (p.id, p.lands_at)).collect();
        if now != self.announced {
            self.announced = now;
            self.send_to_subscribers(
                "pending",
                "pending",
                json!({ "changes": pending.iter().map(pending_json).collect::<Vec<_>>() }),
            );
        }
    }

    /// Everything the server has to say that nobody asked for, since this was
    /// last called, as `(client, message)`. Call it regularly while the clock
    /// advances: it reports changes that landed and, about ten times a second,
    /// the transport and meters.
    pub fn notifications(&mut self) -> Vec<(ClientId, String)> {
        for landed in self.live.take_landed() {
            self.send_to_subscribers("landed", "landed", landed_json(&landed));
        }
        for (control, source) in self.live.take_learned() {
            self.send_to_subscribers(
                "learned",
                "learned",
                json!({ "control": control, "source": source }),
            );
        }
        for notice in self.live.take_notices() {
            self.send_to_subscribers("notices", "notice", json!({ "message": notice }));
        }
        self.queue_changes();

        // the meters are only read from the engine for someone listening
        let listening = self.has_subscribers("meters");
        if listening != self.metering {
            self.metering = listening;
            self.live.set_metering(listening);
        }
        let now = self.live.clock();
        if self
            .last_transport
            .is_none_or(|t| now >= t + self.transport_every)
        {
            self.last_transport = Some(now);
            self.send_to_subscribers(
                "transport",
                "transport",
                transport_json(&self.live.transport()),
            );
            // reading the meters resets them, so only do it for someone listening
            if listening {
                let peaks = self.live.take_peaks();
                self.send_to_subscribers("meters", "meters", json!({ "peaks": peaks }));
            }
        }
        std::mem::take(&mut self.outbox)
    }
}

fn internal_error() -> RpcError {
    RpcError::new(
        INTERNAL_ERROR,
        "niminal hit an internal error handling that; the session carries on",
    )
}

/// What [`Server::begin`] makes of a message.
pub enum Request {
    /// Answered already: send these replies.
    Replies(Vec<String>),
    /// An evaluation or cancellation that needs background compilation.
    Compile(Box<CompileTicket>),
}

/// A request waiting for background compilation.
pub struct CompileTicket {
    id: Option<Value>,
    job: Job,
}

enum Job {
    Eval(Box<EvalJob>),
    Cancel(Box<CancelJob>),
}

enum Done {
    Eval(EvalDone),
    Cancel(CancelDone),
}

impl CompileTicket {
    /// Compile the code. This needs nothing from the server, so it can run
    /// while the render worker continues playing.
    pub fn compile(self) -> CompiledTicket {
        let CompileTicket { id, job } = self;
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
            Job::Eval(job) => Done::Eval(job.compile()),
            Job::Cancel(job) => Done::Cancel(job.compile()),
        }))
        .ok();
        CompiledTicket { id, done }
    }
}

pub struct CompiledTicket {
    id: Option<Value>,
    done: Option<Done>,
}

struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, message: &str) -> RpcError {
        RpcError {
            code,
            message: message.to_string(),
            data: None,
        }
    }
}

fn rejected(problems: &[Problem]) -> RpcError {
    let first = problems
        .first()
        .map_or("the code did not compile", |p| p.message.as_str());
    RpcError {
        code: REJECTED,
        message: first.to_string(),
        data: Some(json!({ "problems": problems.iter().map(problem_json).collect::<Vec<_>>() })),
    }
}

fn optional_u64(params: &Value, key: &str) -> Result<Option<u64>, RpcError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| {
            RpcError::new(INVALID_PARAMS, &format!("`{key}` should be a whole number"))
        }),
    }
}

fn error_reply(id: Value, code: i64, message: &str, data: Option<Value>) -> String {
    let mut error = json!({ "code": code, "message": message });
    if let Some(d) = data {
        error["data"] = d;
    }
    json!({"jsonrpc": "2.0", "id": id, "error": error}).to_string()
}

fn position_json(position: (u64, f64)) -> Value {
    json!({ "bar": position.0, "beat": position.1 })
}

fn accepted_json(session: &Live, a: &Accepted) -> Value {
    json!({
        "id": a.id,
        "lands_at": a.lands_at,
        "position": position_json(a.position),
        "in_seconds": a.lands_at.saturating_sub(session.clock()) as f64 / f64::from(session.sample_rate()),
        "changes": a.changes,
    })
}

fn pending_json(p: &PendingInfo) -> Value {
    json!({ "id": p.id, "lands_at": p.lands_at, "in_samples": p.in_samples, "position": position_json(p.position), "changes": p.changes })
}

fn landed_json(l: &Landed) -> Value {
    json!({ "id": l.id, "at": l.at, "position": position_json(l.position), "changes": l.changes })
}

fn transport_json(t: &Transport) -> Value {
    json!({
        "sample": t.sample, "seconds": t.seconds, "bar": t.bar, "beat": t.beat,
        "bpm": t.bpm, "voices": t.voices, "pending": t.pending,
    })
}

/// A definition plus optional `quantize` on the wire: drop the quantum before deserializing.
fn decode_def<T: serde::de::DeserializeOwned>(params: &Value, kind: &str) -> Result<T, RpcError> {
    let mut value = params.clone();
    if let Some(obj) = value.as_object_mut() {
        obj.remove("quantize");
    }
    serde_json::from_value(value)
        .map_err(|e| RpcError::new(INVALID_PARAMS, &format!("bad {kind}: {e}")))
}

/// Build canonical `.nml` source for a typed performance RPC.
fn api_source(method: &str, params: &Value) -> Result<String, RpcError> {
    let target = || {
        params.get("target").and_then(Value::as_str).ok_or_else(|| {
            RpcError::new(
                INVALID_PARAMS,
                &format!("`{method}` needs a string `target`"),
            )
        })
    };
    Ok(match method {
        "clip.define" => {
            let clip: ClipDef = decode_def(params, "clip")?;
            clip.to_source()
        }
        "scene.define" => {
            let scene: SceneDef = decode_def(params, "scene")?;
            scene.to_source()
        }
        "launch" => {
            if params.get("then").is_some() {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    "`launch` no longer takes `then`: put the follow-up in an arrangement, or launch it later",
                ));
            }
            format!("{}.start", target()?)
        }
        "stop" => format!("{}.stop", target()?),
        "mute" => format!("{}.mute", target()?),
        "unmute" => format!("{}.unmute", target()?),
        "solo" => format!("{}.solo", target()?),
        "unsolo" => match params.get("target").and_then(Value::as_str) {
            Some(t) => format!("{t}.unsolo"),
            None => "unsolo".into(),
        },
        "edit" => {
            let clip = params
                .get("clip")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`edit` needs a string `clip`"))?;
            let change = params
                .get("change")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`edit` needs a string `change`"))?;
            format!("{clip}.{change}")
        }
        other => {
            return Err(RpcError::new(
                METHOD_NOT_FOUND,
                &format!("no method `{other}`"),
            ));
        }
    })
}

fn status_json(live: &mut Live) -> Value {
    let engine = live.engine_status();
    json!({
        "transport": transport_json(&live.transport()),
        "pending": live.pending().iter().map(pending_json).collect::<Vec<_>>(),
        "sample_rate": live.sample_rate(),
        "channels": live.channels(),
        "realtime": engine["realtime"],
        "controls": engine["controls"],
        "instruments": engine["instruments"],
        "scenes": live.scenes().iter().map(|s| json!({
            "name": s.name,
            "clips": s.clips,
            "running": s.running,
            "active": s.active(),
        })).collect::<Vec<_>>(),
        "arrangements": live.defined("arrangement"),
        "defined_clips": live.defined("clip"),
        "clips": live.clips().iter().map(|c| json!({
            "name": c.name,
            "running": c.running,
            "muted": c.muted,
            "soloed": c.soloed,
            "edits": c.edits,
        })).collect::<Vec<_>>(),
    })
}
