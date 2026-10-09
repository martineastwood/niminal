//! The WebSocket front of the daemon: one thread per client, and one that
//! pushes the daemon's notifications out to them.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use niminal_daemon::ClientId;
use serde_json::{Value, json};
use tungstenite::{Error, Message, WebSocket};

use crate::audio::{Shared, lock};

type Outboxes = Arc<Mutex<HashMap<ClientId, Sender<String>>>>;

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
        ServerConfig { listen: IpAddr::from([127, 0, 0, 1]), port: 7400, token: None }
    }
}

pub struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Server {
    pub fn start(daemon: Shared, config: ServerConfig) -> Result<Server, String> {
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
            let (daemon, stop, outboxes) = (daemon.clone(), stop.clone(), outboxes.clone());
            let token = config.token.clone();
            threads.push(thread::spawn(move || {
                let mut clients: Vec<JoinHandle<()>> = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let (daemon, stop, outboxes, token) =
                                (daemon.clone(), stop.clone(), outboxes.clone(), token.clone());
                            clients.push(thread::spawn(move || serve(stream, daemon, outboxes, token, stop)));
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(10)),
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
                    let messages = lock(&daemon).notifications();
                    if !messages.is_empty() {
                        let boxes = outboxes.lock().unwrap_or_else(|e| e.into_inner());
                        for (client, text) in messages {
                            if let Some(tx) = boxes.get(&client) {
                                let _ = tx.send(text);
                            }
                        }
                    }
                    thread::sleep(Duration::from_millis(20));
                }
            }));
        }

        Ok(Server { port, stop, threads })
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

fn serve(stream: TcpStream, daemon: Shared, outboxes: Outboxes, token: Option<String>, stop: Arc<AtomicBool>) {
    // Without a timeout a quiet client would hold this thread in `read` forever.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(20)));
    let Ok(mut ws) = tungstenite::accept(stream) else { return };

    let (tx, rx): (Sender<String>, Receiver<String>) = mpsc::channel();
    let client = lock(&daemon).connect();
    outboxes.lock().unwrap_or_else(|e| e.into_inner()).insert(client, tx);

    let mut authenticated = token.is_none();
    while !stop.load(Ordering::Relaxed) {
        match ws.read() {
            Ok(Message::Text(text)) => {
                let replies = if authenticated {
                    lock(&daemon).handle(client, text.as_str())
                } else {
                    match authenticate(text.as_str(), token.as_deref().unwrap_or_default()) {
                        Ok(()) => {
                            authenticated = true;
                            lock(&daemon).handle(client, text.as_str())
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
            Err(Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        // Whatever the daemon wants to tell this client.
        for text in rx.try_iter() {
            if ws.send(Message::text(text)).is_err() {
                break;
            }
        }
    }
    outboxes.lock().unwrap_or_else(|e| e.into_inner()).remove(&client);
    lock(&daemon).disconnect(client);
    let _ = ws.close(None);
    let _ = ws.flush();
}

/// A client with a token must open with a `hello` that carries it.
fn authenticate(text: &str, token: &str) -> Result<(), String> {
    let request: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let given = request.get("params").and_then(|p| p.get("token")).and_then(Value::as_str);
    if request.get("method").and_then(Value::as_str) == Some("hello") && given == Some(token) {
        return Ok(());
    }
    Err(json!({
        "jsonrpc": "2.0", "id": id,
        "error": {"code": -32000, "message": "this daemon needs a token: say hello with `token`"}
    })
    .to_string())
}

/// A blocking client for tests and the `send` command.
pub struct Client {
    ws: WebSocket<TcpStream>,
    next_id: u64,
}

impl Client {
    pub fn connect(port: u16, token: Option<&str>) -> Result<Client, String> {
        let stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("can't reach the daemon on port {port}: {e}"))?;
        stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
        let (ws, _) = tungstenite::client(format!("ws://127.0.0.1:{port}/"), stream).map_err(|e| e.to_string())?;
        let mut client = Client { ws, next_id: 1 };
        let mut params = json!({"protocol": niminal_daemon::PROTOCOL_VERSION, "client": "niminal-cli"});
        if let Some(t) = token {
            params["token"] = json!(t);
        }
        client.call("hello", params)?;
        Ok(client)
    }

    /// Make a request and wait for its reply, setting aside any notifications
    /// that arrive first. `Err` carries the JSON-RPC error object.
    pub fn call_raw(&mut self, method: &str, params: Value) -> Result<Result<Value, Value>, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.ws.send(Message::text(request.to_string())).map_err(|e| e.to_string())?;
        loop {
            let message = self.read()?;
            if message.get("id") == Some(&json!(id)) {
                return Ok(match message.get("error") {
                    Some(e) => Err(e.clone()),
                    None => Ok(message["result"].clone()),
                });
            }
        }
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.call_raw(method, params)?.map_err(|e| e["message"].as_str().unwrap_or("error").to_string())
    }

    /// The next message from the daemon.
    pub fn read(&mut self) -> Result<Value, String> {
        loop {
            match self.ws.read() {
                Ok(Message::Text(t)) => return serde_json::from_str(t.as_str()).map_err(|e| e.to_string()),
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
