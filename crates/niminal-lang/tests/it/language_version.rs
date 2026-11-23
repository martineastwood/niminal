//! A `niminal 0.1` line is honoured wherever source is read, and a file without one runs as it is.
#![allow(clippy::unwrap_used)]

use niminal_lang::{LANGUAGE_VERSION, compile, parse_composition};

fn error(source: &str) -> String {
    let e = compile(source).err().expect("should not compile");
    format!("{} {}", e[0].message, e[0].help.clone().unwrap_or_default())
}

#[test]
fn a_file_for_this_version_or_older_compiles() {
    let (major, minor) = LANGUAGE_VERSION;
    let body = "instr a() { osc(sine, 220hz) }";
    assert!(compile(body).is_ok());
    assert!(compile(&format!("niminal {major}.{minor}\n{body}")).is_ok());
    assert!(compile(&format!("niminal >={major}.0, <{}.0\n{body}", major + 1)).is_ok());
}

#[test]
fn a_file_for_a_newer_version_says_so() {
    let e = error("niminal 99.0\ninstr a() { osc(sine, 220hz) }");
    assert!(e.contains("this file is for niminal 99.0"), "{e}");
    assert!(e.contains("update niminal"), "{e}");
}

#[test]
fn a_file_for_an_older_range_says_the_language_moved_on() {
    let e = error("niminal <0.1\ninstr a() { osc(sine, 220hz) }");
    assert!(e.contains("the language has changed"), "{e}");
}

#[test]
fn the_composition_parser_honours_it_too() {
    assert!(parse_composition("niminal 99.0\ntempo 120bpm\n").is_err());
    assert!(parse_composition("niminal 0.1\ntempo 120bpm\n").is_ok());
}
