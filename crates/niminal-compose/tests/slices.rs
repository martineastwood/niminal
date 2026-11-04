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

#[test]
fn a_sample_can_be_cut_at_the_hits_it_finds() {
    for args in ["detect: 16", "detect: 16, sensitivity: 0.7"] {
        assert!(
            compile(&format!("{HEAD}chops = pad.slices({args})\n")).is_ok(),
            "{args}"
        );
    }
    for bad in [
        "detect: x",
        "detect: 16, sensitivity: 2",
        "detect: 4, 8",
        "detect: 4, other: 1",
    ] {
        let errors = compile(&format!("{HEAD}chops = pad.slices({bad})\n"))
            .err()
            .unwrap_or_else(|| panic!("{bad}"));
        assert!(
            format!("{errors:?}").contains("sensitivity")
                || format!("{errors:?}").contains("slices"),
            "{bad}: {errors:?}"
        );
    }
}

#[test]
fn a_slice_past_the_end_says_how_many_pieces_there_are() {
    let source = format!(
        "{HEAD}chops = pad.slices(3)\nclip a {{ instrument: chops, notes: [s0 s1 s5] }}\na.start\n"
    );
    let errors = format!("{:?}", compile(&source).err().expect("s5 is past the end"));
    assert!(
        errors.contains("the sliced sample has 3 pieces, `s0` to `s2`"),
        "{errors}"
    );
}
