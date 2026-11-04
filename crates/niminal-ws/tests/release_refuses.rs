//! In a packaged build (`NIMINAL_RELEASE_VERSION` set when it was compiled), `ensure_server` refuses a running server
//! of another release. A development build has no release and uses whatever it finds, so this test does nothing there.

use std::sync::{Arc, Mutex};

use niminal_server::{EngineServer, Session};
use niminal_ws::{Server, ServerConfig, launch::ensure_server};

#[test]
fn ensure_server_refuses() {
    let Some(own) = option_env!("NIMINAL_RELEASE_VERSION") else { return };
    niminal_server::set_release(&format!("{own}-older"));
    let engine = Arc::new(Mutex::new(EngineServer::new(
        Session::new(48_000.0, niminal_lang::Layout::Mono).without_limiter(),
    )));
    let server = Server::start(engine, ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let result = ensure_server(server.port(), None);
    let why = result.unwrap_err();
    assert!(why.contains("older") && why.contains(own) && why.contains("pkill"), "{why}");
}
