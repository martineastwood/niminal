//! The words the editors colour come from the compiler's own list. The editors keep their own copy
//! of it (`editors/codemirror/language.ts`), so this keeps the two from drifting apart.

use std::collections::BTreeSet;

fn set_in(source: &str, name: &str) -> BTreeSet<String> {
    let start = source
        .find(&format!("{name} = new Set('"))
        .unwrap_or_else(|| panic!("language.ts has no `{name}` set"));
    let rest = &source[start..];
    let open = rest.find('\'').unwrap() + 1;
    let close = open + rest[open..].find('\'').unwrap();
    rest[open..close]
        .split_whitespace()
        .map(String::from)
        .collect()
}

#[test]
fn the_editors_colour_the_words_the_compiler_reserves() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../editors/codemirror/language.ts"
    ))
    .expect("the shared editor language is in editors/codemirror");
    let keywords = set_in(&source, "keywords");
    let specials = set_in(&source, "specials");
    let constants = set_in(&source, "constants");
    let types = set_in(&source, "types");
    let editor: BTreeSet<&str> = keywords
        .iter()
        .chain(&specials)
        .chain(&constants)
        .chain(&types)
        .map(String::as_str)
        .collect();
    let compiler: BTreeSet<&str> = niminal_compose::RESERVED_WORDS.iter().copied().collect();

    let uncoloured: Vec<_> = compiler.difference(&editor).collect();
    assert!(
        uncoloured.is_empty(),
        "reserved by the compiler but not coloured by the editors: {uncoloured:?}"
    );
    let unreserved: Vec<_> = keywords
        .iter()
        .map(String::as_str)
        .filter(|w| !compiler.contains(w))
        .collect();
    assert!(
        unreserved.is_empty(),
        "coloured as keywords but not reserved by the compiler: {unreserved:?}"
    );
}
