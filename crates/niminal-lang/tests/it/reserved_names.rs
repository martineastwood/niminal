//! The reserved-words page lists exactly the names the compiler refuses, in the two groups it has.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

/// The names in the backticks of each table of the page, one set per table.
fn tables() -> Vec<BTreeSet<String>> {
    let page = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/src/content/docs/reference/reserved-words.md"
    ))
    .expect("the reserved words page");
    let mut tables: Vec<BTreeSet<String>> = Vec::new();
    let mut in_table = false;
    for line in page.split("## Built-in functions").next().unwrap().lines() {
        if line.starts_with("| Kind") {
            tables.push(BTreeSet::new());
            in_table = true;
        } else if !line.starts_with('|') {
            in_table = false;
        } else if in_table && !line.starts_with("| ---") {
            let names = line.splitn(3, '|').nth(2).unwrap_or("");
            tables.last_mut().unwrap().extend(names.split('`').skip(1).step_by(2).map(String::from));
        }
    }
    tables
}

#[test]
fn the_docs_list_the_reserved_names() {
    let tables = tables();
    assert_eq!(tables.len(), 2, "the page has two tables of names");
    let set = |list: &[&str]| -> BTreeSet<String> { list.iter().map(|s| s.to_string()).collect() };
    assert_eq!(tables[0], set(niminal_lang::EVERYWHERE), "first table and EVERYWHERE disagree");
    assert_eq!(tables[1], set(niminal_lang::TOP_LEVEL), "second table and TOP_LEVEL disagree");
}

#[test]
fn a_reserved_name_is_refused_where_the_docs_say() {
    let refused = |source: &str| niminal_lang::compile(source).is_err();
    for word in niminal_lang::EVERYWHERE.iter().chain(niminal_lang::TOP_LEVEL) {
        // a definition always gets to the name check, so it says why
        let errors = niminal_lang::compile(&format!("const {word} = 1")).err().unwrap_or_default();
        assert!(
            errors.iter().any(|e| e.message.contains("reserved word")),
            "`const {word}` was not refused as a reserved word: {errors:?}"
        );
    }
    // inside a body some words are caught earlier, as a plain syntax error, but all are refused
    for word in niminal_lang::EVERYWHERE {
        for source in [
            format!("instr a(freq: hz) {{ {word} = 1\n osc(sine, freq) * {word} }}"),
            format!("instr a(freq: hz, {word}: 0..1 = 0.5) {{ osc(sine, freq) }}"),
            format!("instr a(freq: hz) {{ for {word} in 0..<2 {{ osc(sine, freq) * {word} }} }}"),
        ] {
            assert!(refused(&source), "`{word}` was accepted in:\n{source}");
        }
    }
    // and the others are free there
    for word in niminal_lang::TOP_LEVEL {
        for source in [
            format!("instr a(freq: hz) {{ {word} = 0.1\n osc(sine, freq) * {word} }}"),
            format!("instr a(freq: hz, {word}: 0..1 = 0.5) {{ osc(sine, freq) * {word} }}"),
        ] {
            let errors = niminal_lang::compile(&source).err().unwrap_or_default();
            assert!(errors.is_empty(), "`{word}` was refused in:\n{source}\n{errors:?}");
        }
    }
}
