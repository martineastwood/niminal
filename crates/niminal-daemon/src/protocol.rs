//! JSON-RPC 2.0 over whatever carries the messages (a WebSocket, in practice).
//!
//! A [`Daemon`] owns a [`Session`] and speaks the protocol to any number of
//! clients: requests come in as text and replies go out as text, and the
//! server pushes notifications (changes landing, the transport, what is
//! waiting) to the clients that subscribed.

use std::collections::{BTreeMap, BTreeSet};

use niminal_score::Event;
use serde_json::{Value, json};

use crate::project::Problem;
use crate::session::{Accepted, Begin, CancelDone, CancelJob, EvalDone, EvalJob, Landed, PendingInfo, ParsedEval, Session, Transport};

/// The protocol version a client must say it speaks.
pub const PROTOCOL_VERSION: u64 = 1;

pub type ClientId = u64;

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
pub const TOPICS: [&str; 5] = ["landed", "pending", "transport", "meters", "notices"];

#[derive(Default)]
struct Client {
    greeted: bool,
    topics: BTreeSet<String>,
}

pub struct Daemon {
    session: Session,
    clients: BTreeMap<ClientId, Client>,
    next_client: ClientId,
    /// The waiting changes last announced, to tell when they change.
    announced: Vec<(u64, u64)>,
    last_transport: Option<u64>,
    transport_every: u64,
    outbox: Vec<(ClientId, String)>,
}

impl Daemon {
    pub fn new(session: Session) -> Daemon {
        let transport_every = (session.sample_rate() / 10.0) as u64;
        Daemon {
            session,
            clients: BTreeMap::new(),
            next_client: 1,
            announced: Vec::new(),
            last_transport: None,
            transport_every,
            outbox: Vec::new(),
        }
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

    pub fn disconnect(&mut self, client: ClientId) {
        self.clients.remove(&client);
    }

    /// Handle one message from `client`; the replies to send back to it.
    pub fn handle(&mut self, client: ClientId, text: &str) -> Vec<String> {
        self.session.sync_realtime();
        let request: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => return vec![error_reply(Value::Null, PARSE_ERROR, &format!("parse error: {e}"), None)],
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
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.dispatch(client, method, &params)))
            .unwrap_or_else(|_| Err(internal_error()));
        self.respond(id, outcome)
    }

    fn respond(&mut self, id: Option<Value>, outcome: Result<Value, RpcError>) -> Vec<String> {
        self.queue_changes();
        match (id, outcome) {
            (None, _) => Vec::new(), // a notification from the client gets no reply
            (Some(id), Ok(result)) => vec![json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()],
            (Some(id), Err(e)) => vec![error_reply(id, e.code, &e.message, e.data)],
        }
    }

    /// Like [`Daemon::handle`], but an `eval` comes back as a ticket to compile
    /// away from the daemon (see [`CompileTicket::compile`]) and hand to
    /// [`Daemon::finish`]. A server does that, so that compiling never holds
    /// up the audio thread. Everything else is answered at once.
    pub fn begin(&mut self, client: ClientId, text: &str) -> Request {
        self.begin_prepared(client, text, Self::prepare_eval(text))
    }

    /// Parse potentially large source before taking the render worker's lock.
    pub fn prepare_eval(text: &str) -> Option<ParsedEval> {
        let request: Value = serde_json::from_str(text).ok()?;
        match request.get("method").and_then(Value::as_str)? {
            "eval" => Some(ParsedEval::new(request["params"]["source"].as_str()?,
                request["params"]["quantize"].as_str())),
            "hush" => Some(ParsedEval::new("hush", Some("now"))),
            _ => None,
        }
    }

    pub fn begin_prepared(&mut self, client: ClientId, text: &str, prepared: Option<ParsedEval>) -> Request {
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
                        return Request::Compile(Box::new(CompileTicket { id: request.get("id").cloned(),
                            job: Job::Cancel(self.session.begin_cancel(id)) }));
                    }
                }
                _ => {}
            }
        }
        let eval = parsed.as_ref().filter(|r| {
            r.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
                && r.get("method").and_then(Value::as_str) == Some("eval")
                && r["params"].get("source").is_some_and(Value::is_string)
                && self.clients.get(&client).is_some_and(|c| c.greeted)
        });
        let Some(request) = eval else { return Request::Replies(self.handle(client, text)) };

        let id = request.get("id").cloned();
        let params = &request["params"];
        let source = params["source"].as_str().unwrap_or_default();
        let quantize = params.get("quantize").and_then(Value::as_str);
        if let Some(dir) = params.get("dir").and_then(Value::as_str) {
            self.session.set_sample_dir(std::path::Path::new(dir));
        }
        let begun = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match prepared {
            Some(parsed) => self.session.begin_parsed(parsed),
            None => self.session.begin_eval(source, quantize),
        }));
        match begun {
            Ok(Begin::Job(job)) => Request::Compile(Box::new(CompileTicket { id, job: Job::Eval(job) })),
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
        self.session.sync_realtime();
        let job = match ticket.done.as_ref()? {
            Done::Eval(done) => Job::Eval(self.session.retry_eval(done)?),
            Done::Cancel(done) => Job::Cancel(self.session.retry_cancel(done)?),
        };
        Some(CompileTicket { id: ticket.id.clone(), job })
    }

    /// Reply to work compiled away from the session. The server checks `retry`
    /// under the same lock before calling this, so no compilation is needed here.
    pub fn finish(&mut self, ticket: CompiledTicket) -> Vec<String> {
        let CompiledTicket { id, done } = ticket;
        let outcome = match done {
            None => Err(internal_error()),
            Some(done) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match done {
                Done::Eval(done) => {
                    let result = self.session.finish_eval(done);
                    self.eval_outcome(result)
                }
                Done::Cancel(done) => Ok(json!({ "cancelled": self.session.finish_cancel(done) })),
            })) {
                Ok(result) => result,
                Err(_) => Err(internal_error()),
            },
        };
        self.respond(id, outcome)
    }

    fn eval_outcome(&self, result: Result<Accepted, Vec<Problem>>) -> Result<Value, RpcError> {
        match result {
            Ok(a) => Ok(accepted_json(&self.session, &a)),
            Err(problems) => Err(rejected(&problems)),
        }
    }

    fn dispatch(&mut self, client: ClientId, method: &str, params: &Value) -> Result<Value, RpcError> {
        if method == "hello" {
            return self.hello(client, params);
        }
        if !self.clients.get(&client).is_some_and(|c| c.greeted) {
            return Err(RpcError::new(PROTOCOL_ERROR, "say hello first, with the protocol version"));
        }
        match method {
            "eval" => self.eval(params),
            "cancel" => {
                let id = optional_u64(params, "id")?;
                Ok(json!({ "cancelled": self.session.cancel(id) }))
            }
            "hush" => match self.session.hush() {
                Ok(a) => Ok(accepted_json(&self.session, &a)),
                Err(problems) => Err(rejected(&problems)),
            },
            "panic" => {
                self.session.panic();
                Ok(json!({}))
            }
            "event.stream" => self.stream(params),
            "control.set" => self.control(params),
            "subscribe" | "unsubscribe" => self.subscribe(client, params, method == "subscribe"),
            "status" => Ok(status_json(&self.session)),
            other => Err(RpcError::new(METHOD_NOT_FOUND, &format!("no method `{other}`"))),
        }
    }

    fn hello(&mut self, client: ClientId, params: &Value) -> Result<Value, RpcError> {
        let version = params.get("protocol").and_then(Value::as_u64);
        if version != Some(PROTOCOL_VERSION) {
            return Err(RpcError::new(
                PROTOCOL_ERROR,
                &format!("this daemon speaks protocol {PROTOCOL_VERSION}, the client said {version:?}"),
            ));
        }
        if let Some(c) = self.clients.get_mut(&client) {
            c.greeted = true;
        }
        Ok(json!({
            "protocol": PROTOCOL_VERSION,
            "sample_rate": self.session.sample_rate(),
            "channels": self.session.channels(),
            "topics": TOPICS,
        }))
    }

    fn eval(&mut self, params: &Value) -> Result<Value, RpcError> {
        let source = params
            .get("source")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`eval` needs a string `source`"))?;
        let quantize = params.get("quantize").and_then(Value::as_str);
        if let Some(dir) = params.get("dir").and_then(Value::as_str) {
            self.session.set_sample_dir(std::path::Path::new(dir));
        }
        let result = self.session.eval(source, quantize);
        self.eval_outcome(result)
    }

    fn control(&mut self, params: &Value) -> Result<Value, RpcError> {
        let name = params.get("name").and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`control.set` needs a string `name`"))?;
        let value = params.get("value").and_then(Value::as_f64)
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`control.set` needs a number `value`"))?;
        let unit = match params.get("unit") {
            None => None,
            Some(value) => Some(value.as_str().ok_or_else(|| RpcError::new(INVALID_PARAMS, "`unit` must be a string"))?),
        };
        let smooth = match params.get("smooth_ms") {
            None => None,
            Some(value) => Some(value.as_f64().ok_or_else(|| RpcError::new(INVALID_PARAMS, "`smooth_ms` must be a number"))?),
        };
        let at = optional_u64(params, "at")?;
        let at = self.session.set_control(name, value, unit, at, smooth).map_err(|p| rejected(&p))?;
        Ok(json!({ "at": at }))
    }

    fn stream(&mut self, params: &Value) -> Result<Value, RpcError> {
        let events: Vec<Event> = params
            .get("events")
            .cloned()
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`event.stream` needs `events`"))
            .and_then(|v| serde_json::from_value(v).map_err(|e| RpcError::new(INVALID_PARAMS, &format!("bad events: {e}"))))?;
        self.session.stream_events(events).map_err(|problems| rejected(&problems))?;
        Ok(json!({}))
    }

    fn subscribe(&mut self, client: ClientId, params: &Value, on: bool) -> Result<Value, RpcError> {
        let topics: Vec<String> = params
            .get("topics")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect())
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`topics` should be a list of names"))?;
        for t in &topics {
            if !TOPICS.contains(&t.as_str()) {
                return Err(RpcError::new(INVALID_PARAMS, &format!("no topic `{t}`; the topics are {}", TOPICS.join(", "))));
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
        self.clients.values().any(|c| c.greeted && c.topics.contains(topic))
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
        let pending = self.session.pending();
        let now: Vec<(u64, u64)> = pending.iter().map(|p| (p.id, p.lands_at)).collect();
        if now != self.announced {
            self.announced = now;
            self.send_to_subscribers("pending", "pending", json!({ "changes": pending.iter().map(pending_json).collect::<Vec<_>>() }));
        }
    }

    /// Everything the server has to say that nobody asked for, since this was
    /// last called, as `(client, message)`. Call it regularly while the clock
    /// advances: it reports changes that landed and, about ten times a second,
    /// the transport and meters.
    pub fn notifications(&mut self) -> Vec<(ClientId, String)> {
        self.session.sync_realtime();
        for landed in self.session.take_landed() {
            self.send_to_subscribers("landed", "landed", landed_json(&landed));
        }
        for notice in self.session.take_notices() {
            self.send_to_subscribers("notices", "notice", json!({ "message": notice }));
        }
        let silenced = self.session.take_silenced();
        if silenced > 0 {
            self.send_to_subscribers(
                "notices",
                "notice",
                json!({ "message": format!("{silenced} voice(s) produced NaN or infinity and were silenced") }),
            );
        }
        self.queue_changes();

        let now = self.session.clock();
        if self.last_transport.is_none_or(|t| now >= t + self.transport_every) {
            self.last_transport = Some(now);
            self.send_to_subscribers("transport", "transport", transport_json(&self.session.transport()));
            // reading the meters resets them, so only do it for someone listening
            if self.has_subscribers("meters") {
                let peaks = self.session.take_peaks();
                self.send_to_subscribers("meters", "meters", json!({ "peaks": peaks }));
            }
        }
        std::mem::take(&mut self.outbox)
    }
}

fn internal_error() -> RpcError {
    RpcError::new(INTERNAL_ERROR, "niminal hit an internal error handling that; the session carries on")
}

/// What [`Daemon::begin`] makes of a message.
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
    /// Compile the code. This needs nothing from the daemon, so it can run
    /// while the render worker continues playing.
    pub fn compile(self) -> CompiledTicket {
        let CompileTicket { id, job } = self;
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
            Job::Eval(job) => Done::Eval(job.compile()),
            Job::Cancel(job) => Done::Cancel(job.compile()),
        })).ok();
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
        RpcError { code, message: message.to_string(), data: None }
    }
}

fn rejected(problems: &[Problem]) -> RpcError {
    let first = problems.first().map_or("the code did not compile", |p| p.message.as_str());
    RpcError {
        code: REJECTED,
        message: first.to_string(),
        data: Some(json!({ "problems": problems.iter().map(problem_json).collect::<Vec<_>>() })),
    }
}

fn optional_u64(params: &Value, key: &str) -> Result<Option<u64>, RpcError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| RpcError::new(INVALID_PARAMS, &format!("`{key}` should be a whole number"))),
    }
}

fn error_reply(id: Value, code: i64, message: &str, data: Option<Value>) -> String {
    let mut error = json!({ "code": code, "message": message });
    if let Some(d) = data {
        error["data"] = d;
    }
    json!({"jsonrpc": "2.0", "id": id, "error": error}).to_string()
}

fn problem_json(p: &Problem) -> Value {
    json!({
        "message": p.message,
        "help": p.help,
        "line": p.line,
        "column": p.column,
        "span": p.span.map(|s| json!({"start": s.start, "end": s.end})),
        "in_definition": p.in_definition,
    })
}

fn position_json(position: (u64, f64)) -> Value {
    json!({ "bar": position.0, "beat": position.1 })
}

fn accepted_json(session: &Session, a: &Accepted) -> Value {
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

fn status_json(session: &Session) -> Value {
    json!({
        "transport": transport_json(&session.transport()),
        "pending": session.pending().iter().map(pending_json).collect::<Vec<_>>(),
        "sample_rate": session.sample_rate(),
        "channels": session.channels(),
        "realtime": session.realtime_metrics().map(|m| json!({
            "prepared_until": session.prepared_until(),
            "late_events": m.late_events, "starved_frames": m.starved_frames,
            "capacity_drops": m.capacity_drops, "control_drops": m.control_drops, "retire_pressure": m.retire_pressure,
            "deadline_misses": m.deadline_misses, "max_render_micros": m.max_render_micros,
        })),
        "controls": session.control_values().iter().map(|(name, value, target, unit)| {
            let display = |value: f32| if *unit == Some("db") { 20.0 * f64::from(value.max(f32::MIN_POSITIVE)).log10() } else { f64::from(value) };
            json!({ "name": name, "value": display(*value), "target": display(*target), "unit": unit })
        }).collect::<Vec<_>>(),
        "scenes": session.defined("scene"),
        "clips": session.defined("clip"),
        "tracks": session.tracks().iter().map(|t| json!({
            "name": t.name, "clip": t.clip, "muted": t.muted, "soloed": t.soloed,
        })).collect::<Vec<_>>(),
    })
}
