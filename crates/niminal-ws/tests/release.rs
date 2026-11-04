//! An app of one release will not use a server left running from another.

use niminal_ws::launch::release_mismatch;

#[test]
fn the_same_release_is_used() {
    assert_eq!(release_mismatch("0.0.1-alpha.10", Some("0.0.1-alpha.10")), None);
}

#[test]
fn another_release_is_refused_and_the_message_says_what_to_do() {
    let why = release_mismatch("0.0.1-alpha.10", Some("0.0.1-alpha.9")).unwrap();
    assert!(why.contains("release 0.0.1-alpha.9") && why.contains("alpha.10") && why.contains("pkill"), "{why}");
}

#[test]
fn a_server_that_does_not_say_which_release_it_is_older() {
    let why = release_mismatch("0.0.1-alpha.10", None).unwrap();
    assert!(why.contains("older release"), "{why}");
}

#[test]
fn a_server_tells_every_client_which_release_it_is() {
    use std::sync::{Arc, Mutex};

    use niminal_server::{EngineServer, Session};
    use niminal_ws::{Client, Server, ServerConfig};

    niminal_server::set_release("0.0.1-test");
    let engine = Arc::new(Mutex::new(EngineServer::new(
        Session::new(48_000.0, niminal_lang::Layout::Mono).without_limiter(),
    )));
    let server = Server::start(engine, ServerConfig { port: 0, ..ServerConfig::default() }).unwrap();
    let (_client, hello) = Client::connect_as(server.port(), None, "release-test").unwrap();
    assert_eq!(hello["release"], "0.0.1-test");
}
