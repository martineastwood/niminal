//! `use rig` pulls another file's definitions into this one. The walk over the files is
//! [`niminal_lang::expand_with_rules`], shared with the engine; what is here is what the sequencer's
//! language adds to it: its own kinds of definition to rename, and its own lexer.
//!
//! Names are kept apart by renaming. `use rig as r` renames what `rig` defines
//! to `r::name`, and a module's `_private` names to `rig::_name`, which no
//! other file may write.

use std::path::Path;

use crate::ide::{SymbolKind, symbols};
use crate::lexer::{Tok, lex};
use niminal_lang::{Diagnostic, Expanded, ModuleRules, Span};

/// Resolve the `use` lines of `source`, which lives in `dir`. A module is
/// found next to the file that uses it, then in `lib` under `dir`, then among
/// the installed packages (`$NIMINAL_PACKAGES`, or `~/.niminal/packages`).
pub fn expand(source: &str, dir: &Path) -> Result<Expanded, Diagnostic> {
    niminal_lang::expand_with_rules(source, dir, niminal_lang::packages_dir(), &ComposeRules)
}

struct ComposeRules;

impl ModuleRules for ComposeRules {
    fn rename(&self, source: &str, module: &str, alias: Option<&str>) -> String {
        rename(source, module, alias)
    }

    fn check_private(&self, source: &str, at: Option<Span>) -> Result<(), Diagnostic> {
        let Ok(tokens) = lex(source) else {
            return Ok(());
        };
        let words: Vec<Option<(String, Span)>> = tokens
            .into_iter()
            .map(|t| match t.tok {
                Tok::Ident(name) => Some((name, t.span)),
                _ => None,
            })
            .collect();
        niminal_lang::check_private_words(&words, at)
    }
}

/// Rename what a module defines so it cannot meet other files' names: public
/// names get the alias (when there is one), private names the module's name.
fn rename(source: &str, module: &str, alias: Option<&str>) -> String {
    let (Ok(defs), Ok(tokens)) = (symbols(source), lex(source)) else {
        return source.to_string();
    };
    let mut edits = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        let Tok::Ident(name) = &t.tok else { continue };
        let Some(def) = defs.iter().find(|d| &d.name == name) else {
            continue;
        };
        let Some(prefix) = (if name.starts_with('_') {
            Some(module)
        } else {
            alias
        }) else {
            continue;
        };
        // `x.name` calls an opcode, and `name:` is an argument unless it is a part in a scene
        let after_dot = i > 0 && tokens[i - 1].tok == Tok::Dot;
        let before_colon = tokens.get(i + 1).is_some_and(|n| n.tok == Tok::Colon);
        if (after_dot && def.kind != SymbolKind::Function)
            || (before_colon && def.kind != SymbolKind::Clip)
        {
            continue;
        }
        edits.push((t.span, format!("{prefix}::{name}")));
    }
    let mut out = source.to_string();
    for (span, text) in edits.into_iter().rev() {
        out.replace_range(span.start..span.end, &text);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn project(files: &[(&str, &str)]) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "niminal-modules-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        for (name, text) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir.canonicalize().unwrap()
    }

    fn expand_here(source: &str, dir: &Path) -> Result<Expanded, Diagnostic> {
        niminal_lang::expand_with_rules(source, dir, None, &ComposeRules)
    }

    #[test]
    fn a_module_is_added_once_after_the_file() {
        let dir = project(&[
            ("a.nml", "use c\nA\n"),
            ("b.nml", "use c\nB\n"),
            ("c.nml", "C\n"),
        ]);
        let e = expand_here("use a\nuse b\nROOT\n", &dir).unwrap();
        assert!(e.text.starts_with("     \n     \nROOT\n"));
        assert_eq!(e.text.matches("C\n").count(), 1);
        let (c, a, b) = (
            e.text.find("C\n").unwrap(),
            e.text.find("A\n").unwrap(),
            e.text.find("B\n").unwrap(),
        );
        assert!(c < a && a < b);
        assert_eq!(e.files().count(), 3);
    }

    #[test]
    fn modules_are_found_in_lib_subfolders_and_packages() {
        let dir = project(&[("lib/rig.nml", "RIG\n"), ("score/verse.nml", "VERSE\n")]);
        let packages = project(&[("acme.nml", "ACME\n")]);
        let e = niminal_lang::expand_with_rules(
            "use rig\nuse score/verse\nuse acme\n",
            &dir,
            Some(packages),
            &ComposeRules,
        )
        .unwrap();
        assert!(e.text.contains("RIG") && e.text.contains("VERSE") && e.text.contains("ACME"));
    }

    #[test]
    fn circular_and_missing_uses_are_errors() {
        let dir = project(&[("a.nml", "use b\n"), ("b.nml", "use a\n")]);
        let err = expand_here("use a\n", &dir).err().unwrap();
        assert!(err.message.contains("uses itself"), "{}", err.message);
        let err = expand_here("x\nuse nope\n", &dir).err().unwrap();
        assert_eq!(err.message, "no module `nope`");
        assert_eq!(err.span, Span::new(2, 10));
    }

    #[test]
    fn an_error_in_a_module_points_at_the_use_line() {
        let dir = project(&[("rig.nml", "ok\nbad\n")]);
        let e = expand_here("use rig\nplay\n", &dir).unwrap();
        let at = e.text.find("bad").unwrap();
        let d = e.locate(Diagnostic::new("what", Span::new(at, at + 3)));
        assert_eq!(d.message, "in rig.nml line 2: what");
        assert_eq!(d.span, Span::new(0, 7));
        let own = Diagnostic::new("mine", Span::new(8, 12));
        assert_eq!(e.locate(own.clone()), own);
    }

    const RIG: &str = "fn shimmer(x, size: 0..1 = 0.5) { x }\ninstr tone(freq: hz) { osc(sine, freq).shimmer(size: 0.2) }\nclip lead { instrument: tone }\nscene a { lead: riff }\nriff = [c4]\n_secret = [d4]\n";

    #[test]
    fn an_alias_renames_what_the_module_defines() {
        let dir = project(&[("rig.nml", RIG)]);
        let e = expand_here("use rig as r\nosc(saw, 1hz).r::shimmer(size: 0.1)\n", &dir).unwrap();
        let module = &e.text[e.root_len..];
        assert!(module.contains("fn r::shimmer(x, size:"), "{module}");
        assert!(module.contains(".r::shimmer(size: 0.2)"), "{module}");
        assert!(
            module.contains("instr r::tone(freq: hz)") && module.contains("instrument: r::tone"),
            "{module}"
        );
        assert!(
            module.contains("scene r::a { r::lead: r::riff }"),
            "{module}"
        );
        assert!(
            module.contains("r::riff = [c4]") && module.contains("rig::_secret = [d4]"),
            "{module}"
        );
        // arguments keep their names
        assert!(module.contains("size: 0.2") && module.contains("(freq: hz)"));
        let imports: Vec<_> = e.imports().collect();
        assert_eq!(imports[0].alias, Some("r"));
        assert!(imports[0].source.contains("fn shimmer"));
    }

    #[test]
    fn without_an_alias_only_private_names_are_renamed() {
        let dir = project(&[("rig.nml", RIG)]);
        let e = expand_here("use rig\n", &dir).unwrap();
        let module = &e.text[e.root_len..];
        assert!(
            module.contains("fn shimmer(") && module.contains("rig::_secret"),
            "{module}"
        );
    }

    #[test]
    fn private_names_cannot_be_written_from_outside() {
        let dir = project(&[("rig.nml", RIG)]);
        let err = expand_here("use rig\nx.__update(rig::_secret)\n", &dir)
            .err()
            .unwrap();
        assert_eq!(err.message, "`_secret` is private to `rig`");
    }

    #[test]
    fn a_module_cannot_be_used_under_two_names() {
        let dir = project(&[("rig.nml", RIG)]);
        assert!(expand_here("use rig as a\nuse rig as a\n", &dir).is_ok());
        let err = expand_here("use rig as a\nuse rig as b\n", &dir)
            .err()
            .unwrap();
        assert!(err.message.contains("different names"), "{}", err.message);
        let err = expand_here("use rig as 1x\n", &dir).err().unwrap();
        assert!(err.message.contains("isn't a name"), "{}", err.message);
    }
}
