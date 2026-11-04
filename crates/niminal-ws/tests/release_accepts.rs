//! In a packaged build (`NIMINAL_RELEASE_VERSION` set when it was compiled), `ensure_server` accepts a running server
//! of the same release. A development build has no release and uses whatever it finds, so this test does nothing there.

use std::sync::{Arc, Mutex};

use niminal_server::{EngineServer, Session};
use niminal_ws::{Server, ServerConfig, launch::ensure_server};

#[test]
fn ensure_server_accepts() {
    let Some(own) = option_env!("NIMINAL_RELEASE_VERSION") else { return };
    niminal_server::set_release(own);
    let engine = Arc::new(Mutex::new(EngineServer::new(
        Session::new(48_000.0, niminal_lang::Layout::Mono).without_limiter(),
    )));
    let server = Server::start(engine, ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let result = ensure_server(server.port(), None);
    assert_eq!(result, Ok(()));
}
