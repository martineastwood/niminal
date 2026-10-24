//! `name.slices([...])` at the top level of a file, where the lexer keeps a bracketed list as raw text.

use niminal_compose::{SymbolKind, compile, symbols};

const HEAD: &str = "const wave = harmonics([1], size: 1000)\npad = sample(wave)\n";

#[test]
fn a_sample_can_be_cut_at_places_given_as_a_list() {
    for places in ["[0, 0.25, 0.6]", "[0,0.25,0.6]", "[0, 0.25, 0.6,]"] {
        let source = format!("{HEAD}chops = pad.slices({places})\n");
        assert!(
            compile(&source).is_ok(),
            "{places}: {:?}",
            compile(&source).err()
        );
        let kit = symbols(&source)
            .unwrap()
            .into_iter()
            .find(|s| s.name == "chops")
            .unwrap();
        assert_eq!(
            (kit.kind, kit.params.len()),
            (SymbolKind::Kit, 3),
            "{places}"
        );
    }
    for bad in ["[0 0.25 0.6]", "[0, x]", "[]", "[0, 0.5s]", "0.5"] {
        let errors = compile(&format!("{HEAD}chops = pad.slices({bad})\n"))
            .err()
            .unwrap_or_else(|| panic!("{bad}"));
        assert!(
            format!("{errors:?}").contains("slices"),
            "{bad}: {errors:?}"
        );
    }
    assert!(compile(&format!("{HEAD}chops = pad.slices(4)\n")).is_ok());
}
