//! JSON-RPC over WebSocket: one thread per client, and one that pushes a service's notifications
//! out to them. The server is a service, and so is any client that serves an API of its own.

pub mod worker;

use std::collections::{HashMap, VecDeque};
use std::io::ErrorKind;
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use niminal_server::{ClientId, EngineServer};
use serde_json::{Value, json};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Error, Message, WebSocket};

type Outboxes = Arc<Mutex<HashMap<ClientId, Arc<Mutex<Outbox>>>>>;
#[derive(Default)]
struct Outbox {
    queue: VecDeque<(Option<String>, String)>,
    bytes: usize,
    closed: bool,
    dropped: u64,
}
impl Outbox {
    fn push(&mut self, text: String) {
        if self.closed {
            return;
        }
        let value: Value = serde_json::from_str(&text).unwrap_or_default();
        let key = match value["method"].as_str() {
            Some("clock" | "meters") => value["method"].as_str().map(str::to_string),
            Some("scope") => Some(format!("scope:{}", value["params"]["id"])),
            _ => None,
        };
        if let Some(key) = &key
            && let Some((_, old)) = self.queue.iter_mut().find(|(k, _)| k.as_ref() == Some(key)) {
                if self.bytes - old.len() + text.len() > 4 * 1024 * 1024 {
                    self.dropped += 1;
                    return;
                }
                self.bytes -= old.len();
                *old = text;
                self.bytes += old.len();
                self.dropped += 1;
                return;
            }
        if self.bytes + text.len() > 4 * 1024 * 1024 || self.queue.len() >= 256 {
            if key.is_some() {
                self.dropped += 1;
            } else {
                self.closed = true;
            }
            return;
        }
        self.bytes += text.len();
        self.queue.push_back((key, text));
    }
    fn take(&mut self) -> Vec<String> {
        let dropped = std::mem::take(&mut self.dropped);
        self.bytes = 0;
        self.queue
            .drain(..)
            .map(|(_, text)| {
                if dropped == 0 {
                    text
                } else {
                    let mut value: Value = serde_json::from_str(&text).unwrap_or_default();
                    if value["params"].is_object() {
                        value["params"]["transport_dropped"] = json!(dropped);
                    }
                    value.to_string()
                }
            })
            .collect()
    }
}

/// The most clients at once, the largest message (source code, mostly) and
/// how long a new connection has to complete its handshake.
const MAX_CLIENTS: usize = 16;
const MAX_MESSAGE: usize = 4 << 20;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
/// Client threads run the compiler, so they get more stack than the default.
const CLIENT_STACK: usize = 32 << 20;

/// How the server is run.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// The address to listen on. Only local addresses are allowed until
    /// connections can be encrypted.
    pub listen: IpAddr,
    pub port: u16,
    /// If set, a client must give this token in its `hello`.
    pub token: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            listen: IpAddr::from([127, 0, 0, 1]),
            port: 7400,
            token: None,
        }
    }
}

/// What a server serves: something that answers JSON-RPC requests for any number of clients and pushes
/// notifications to them. A request that needs compiling is answered in three steps, so that the compiling
/// can happen without the service locked: `begin`, `compile`, `finish`.
pub trait Service: Send + Sized + 'static {
    /// What can be done with a request before the service is locked.
    type Prepared: Send;
    /// Work to do away from the lock, and its result.
    type Job: Send;
    type Done: Send;

    fn connect(&mut self) -> ClientId;
    fn is_connected(&self, _client: ClientId) -> bool {
        true
    }
    fn disconnect(&mut self, client: ClientId);
    fn notifications(&mut self) -> Vec<(ClientId, String)>;
    fn prepare(text: &str) -> Option<Self::Prepared>;
    /// Answer at once, or hand back work.
    fn begin(
        &mut self,
        client: ClientId,
        text: &str,
        prepared: Option<Self::Prepared>,
    ) -> Begun<Self::Job>;
    fn compile(job: Self::Job) -> Self::Done;
    /// Work to do again because the service changed while it was being done.
    fn retry(&mut self, done: &Self::Done) -> Option<Self::Job>;
    fn finish(&mut self, done: Self::Done) -> Vec<String>;
    /// Read what a request will need, such as sample files, without the service locked.
    fn preload(_service: &Mutex<Self>, _text: &str) {}
}

pub enum Begun<J> {
    Replies(Vec<String>),
    Job(J),
}

impl Service for EngineServer {
    type Prepared = ();
    type Job = ();
    type Done = ();

    fn is_connected(&self, client: ClientId) -> bool {
        EngineServer::is_connected(self, client)
    }
    fn connect(&mut self) -> ClientId {
        EngineServer::connect(self)
    }
    fn disconnect(&mut self, client: ClientId) {
        EngineServer::disconnect(self, client);
    }
    fn notifications(&mut self) -> Vec<(ClientId, String)> {
        EngineServer::notifications(self)
    }
    fn prepare(_: &str) -> Option<()> {
        None
    }
    fn begin(&mut self, client: ClientId, text: &str, _: Option<()>) -> Begun<()> {
        Begun::Replies(self.handle(client, text))
    }
    fn compile(_: ()) {}
    fn retry(&mut self, _: &()) -> Option<()> {
        None
    }
    fn finish(&mut self, _: ()) -> Vec<String> {
        Vec::new()
    }
}

/// Lock a service, carrying on if another thread panicked while holding it.
pub fn lock<S>(service: &Mutex<S>) -> std::sync::MutexGuard<'_, S> {
    service.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Server {
    pub fn start<S: Service>(
        server: Arc<Mutex<S>>,
        config: ServerConfig,
    ) -> Result<Server, String> {
        if !config.listen.is_loopback() {
            return Err(format!(
                "won't listen on {}: connections from other machines need a token and TLS, and TLS isn't supported yet",
                config.listen
            ));
        }
        let listener = TcpListener::bind((config.listen, config.port))
            .map_err(|e| format!("can't listen on {}:{}: {e}", config.listen, config.port))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();

        let stop = Arc::new(AtomicBool::new(false));
        let outboxes: Outboxes = Arc::default();
        let mut threads = Vec::new();

        // Accept clients.
        {
            let (server, stop, outboxes) = (server.clone(), stop.clone(), outboxes.clone());
            let token = config.token.clone();
            threads.push(thread::spawn(move || {
                let mut clients: Vec<JoinHandle<()>> = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    clients.retain(|c| !c.is_finished());
                    match listener.accept() {
                        Ok((stream, _)) if clients.len() >= MAX_CLIENTS => drop(stream),
                        Ok((stream, _)) => {
                            let (server, stop, outboxes, token) = (
                                server.clone(),
                                stop.clone(),
                                outboxes.clone(),
                                token.clone(),
                            );
                            // Compiling happens on this thread, and deeply nested code needs room.
                            let spawned = thread::Builder::new()
                                .stack_size(CLIENT_STACK)
                                .spawn(move || serve(stream, server, outboxes, token, stop));
                            if let Ok(handle) = spawned {
                                clients.push(handle);
                            }
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(25))
                        }
                        Err(_) => break,
                    }
                }
                for c in clients {
                    let _ = c.join();
                }
            }));
        }

        // Push notifications.
        {
            let (stop, outboxes) = (stop.clone(), outboxes.clone());
            threads.push(thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let messages = lock(&server).notifications();
                    if !messages.is_empty() {
                        let boxes = outboxes.lock().unwrap_or_else(|e| e.into_inner());
                        for (client, text) in messages {
                            if let Some(tx) = boxes.get(&client) {
                                tx.lock().unwrap_or_else(|e| e.into_inner()).push(text);
                            }
                        }
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }));
        }

        Ok(Server {
            port,
            stop,
            threads,
        })
    }

    /// The port actually listening (useful when asked for port 0).
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn stop(mut self) {
        self.shut_down();
    }

    fn shut_down(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shut_down();
    }
}

fn serve<S: Service>(
    stream: TcpStream,
    server: Arc<Mutex<S>>,
    outboxes: Outboxes,
    token: Option<String>,
    stop: Arc<AtomicBool>,
) {
    // A client that connects and says nothing must not hold this thread for ever.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE));
    let Ok(mut ws) = tungstenite::accept_with_config(stream, Some(config)) else {
        return;
    };
    // Then reads wake up often, to notice the server stopping and to send notifications.
    let _ = ws
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(20)));

    let outbox = Arc::new(Mutex::new(Outbox::default()));
    let client = lock(&server).connect();
    outboxes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(client, outbox.clone());

    let mut authenticated = token.is_none();
    while !stop.load(Ordering::Relaxed) {
        if !lock(&server).is_connected(client)
            || outbox.lock().unwrap_or_else(|e| e.into_inner()).closed
        {
            break;
        }
        match ws.read() {
            Ok(Message::Text(text)) => {
                let replies = if authenticated {
                    request(&server, client, text.as_str())
                } else {
                    match authenticate(text.as_str(), token.as_deref().unwrap_or_default()) {
                        Ok(()) => {
                            authenticated = true;
                            request(&server, client, text.as_str())
                        }
                        Err(reply) => vec![reply],
                    }
                };
                for reply in replies {
                    if ws.send(Message::text(reply)).is_err() {
                        break;
                    }
                }
            }
            Ok(Message::Close(_)) | Err(Error::ConnectionClosed | Error::AlreadyClosed) => break,
            Ok(_) => {}
            Err(Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        // Whatever the server wants to tell this client.
        let messages = outbox.lock().unwrap_or_else(|e| e.into_inner()).take();
        for text in messages {
            if ws.send(Message::text(text)).is_err() {
                break;
            }
        }
    }
    outboxes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&client);
    lock(&server).disconnect(client);
    let _ = ws.close(None);
    let _ = ws.flush();
}

/// Answer one message. Source parsing, sample loading and compilation happen
/// outside the service's lock; only snapshots and acceptance take it.
fn request<S: Service>(service: &Arc<Mutex<S>>, client: ClientId, text: &str) -> Vec<String> {
    S::preload(service, text);
    let prepared = S::prepare(text);
    let begun = lock(service).begin(client, text, prepared);
    match begun {
        Begun::Replies(replies) => replies,
        Begun::Job(job) => {
            let mut done = S::compile(job);
            loop {
                let retry = {
                    let mut s = lock(service);
                    match s.retry(&done) {
                        Some(job) => job,
                        None => return s.finish(done),
                    }
                };
                done = S::compile(retry);
            }
        }
    }
}

/// A client with a token must open with a `hello` that carries it.
fn authenticate(text: &str, token: &str) -> Result<(), String> {
    let request: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let given = request
        .get("params")
        .and_then(|p| p.get("token"))
        .and_then(Value::as_str);
    if request.get("method").and_then(Value::as_str) == Some("hello") && given == Some(token) {
        return Ok(());
    }
    Err(json!({
        "jsonrpc": "2.0", "id": id,
        "error": {"code": -32000, "message": "this server needs a token: say hello with `token`"}
    })
    .to_string())
}

/// A blocking client for tests and the `send` command.
pub struct Client {
    ws: WebSocket<TcpStream>,
    next_id: u64,
    /// Notifications that arrived while waiting for a reply.
    notifications: Vec<Value>,
}

impl Client {
    pub fn connect(port: u16, token: Option<&str>) -> Result<Client, String> {
        let stream = TcpStream::connect(("127.0.0.1", port))
            .map_err(|e| format!("can't reach the server on port {port}: {e}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| e.to_string())?;
        let (ws, _) = tungstenite::client(format!("ws://127.0.0.1:{port}/"), stream)
            .map_err(|e| e.to_string())?;
        let mut client = Client {
            ws,
            next_id: 1,
            notifications: Vec::new(),
        };
        let mut params =
            json!({"protocol": niminal_server::PROTOCOL_VERSION, "client": "niminal-cli"});
        if let Some(t) = token {
            params["token"] = json!(t);
        }
        client.call("hello", params)?;
        Ok(client)
    }

    /// Make a request and wait for its reply, setting aside any notifications
    /// that arrive first. `Err` carries the JSON-RPC error object.
    pub fn call_raw(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Result<Value, Value>, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.ws
            .send(Message::text(request.to_string()))
            .map_err(|e| e.to_string())?;
        loop {
            let message = self.read()?;
            if message.get("id") == Some(&json!(id)) {
                return Ok(match message.get("error") {
                    Some(e) => Err(e.clone()),
                    None => Ok(message["result"].clone()),
                });
            }
            if message.get("method").is_some() {
                self.notifications.push(message);
            }
        }
    }

    /// The notifications that arrived while waiting for replies, oldest first.
    pub fn take_notifications(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.notifications)
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.call_raw(method, params)?
            .map_err(|e| e["message"].as_str().unwrap_or("error").to_string())
    }

    /// The next message from the server.
    pub fn read(&mut self) -> Result<Value, String> {
        loop {
            match self.ws.read() {
                Ok(Message::Text(t)) => {
                    return serde_json::from_str(t.as_str()).map_err(|e| e.to_string());
                }
                Ok(_) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }

    /// The next notification called `method`, within `timeout`.
    pub fn wait_for(&mut self, method: &str, timeout: Duration) -> Result<Value, String> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if std::time::Instant::now() > deadline {
                return Err(format!("no `{method}` notification arrived in time"));
            }
            let message = self.read()?;
            if message.get("method").and_then(Value::as_str) == Some(method) {
                return Ok(message["params"].clone());
            }
        }
    }
}
