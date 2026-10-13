//! `use rig` pulls another file's definitions into this one. The files are
//! joined into one source text before it is parsed: each module once, after
//! the file that uses it, so spans in the file itself stay where they were.
//!
//! Names are kept apart by renaming. `use rig as r` renames what `rig` defines
//! to `r::name`, and a module's `_private` names to `rig::_name`, which no
//! other file may write.

use std::path::{Path, PathBuf};

use crate::diag::{Diagnostic, Span};
use crate::ide::{SymbolKind, symbols};
use crate::lexer::{Tok, lex};

/// A source with its `use` lines resolved.
pub struct Expanded {
    /// The file's own text, then each module it uses, with the `use` lines blanked.
    pub text: String,
    /// How much of `text` is the file itself.
    pub root_len: usize,
    modules: Vec<Module>,
}

struct Module {
    path: PathBuf,
    label: String,
    alias: Option<String>,
    /// The module as written, with its own `use` lines blanked.
    source: String,
    /// Where the module's text starts and ends in `Expanded::text`.
    start: usize,
    end: usize,
    /// The `use` line in the file itself that brought it in.
    used_at: Span,
}

/// A module that was joined in, as the file it came from.
pub struct Import<'a> {
    pub path: &'a Path,
    /// The `y` of `use x as y`; the module's names are only written `y::name`.
    pub alias: Option<&'a str>,
    /// The file's text, with its `use` lines blanked, so offsets match the file.
    pub source: &'a str,
}

impl Expanded {
    /// The module files that were read.
    pub fn files(&self) -> impl Iterator<Item = &Path> {
        self.modules.iter().map(|m| m.path.as_path())
    }

    pub fn imports(&self) -> impl Iterator<Item = Import<'_>> {
        self.modules.iter().map(|m| Import {
            path: &m.path,
            alias: m.alias.as_deref(),
            source: &m.source,
        })
    }

    /// Point a diagnostic from a module at the `use` line that brought the
    /// module in, naming the module's own file and line.
    pub fn locate(&self, mut d: Diagnostic) -> Diagnostic {
        if d.span.start < self.root_len {
            return d;
        }
        let Some(m) = self
            .modules
            .iter()
            .find(|m| d.span.start >= m.start && d.span.start <= m.end)
        else {
            return d;
        };
        let line = self.text[m.start..d.span.start].matches('\n').count() + 1;
        d.message = format!("in {} line {line}: {}", m.label, d.message);
        d.span = m.used_at;
        d
    }
}

/// Resolve the `use` lines of `source`, which lives in `dir`. A module is
/// found next to the file that uses it, then in `lib` under `dir`, then among
/// the installed packages (`$NIMINAL_PACKAGES`, or `~/.niminal/packages`).
pub fn expand(source: &str, dir: &Path) -> Result<Expanded, Diagnostic> {
    expand_with(source, dir, packages_dir())
}

fn packages_dir() -> Option<PathBuf> {
    std::env::var_os("NIMINAL_PACKAGES")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".niminal").join("packages"))
        })
}

fn expand_with(
    source: &str,
    dir: &Path,
    packages: Option<PathBuf>,
) -> Result<Expanded, Diagnostic> {
    let mut walk = Walk {
        root_dir: dir,
        packages,
        stack: Vec::new(),
        done: Vec::new(),
    };
    let mut text = walk.visit(source, dir, None)?;
    let root_len = text.len();
    let mut modules = Vec::new();
    for done in walk.done {
        let body = rename(&done.source, &done.name, done.alias.as_deref());
        text.push('\n');
        let start = text.len();
        text.push_str(&body);
        modules.push(Module {
            path: done.path,
            label: format!("{}.nml", done.name),
            alias: done.alias,
            source: done.source,
            start,
            end: text.len(),
            used_at: done.used_at,
        });
    }
    Ok(Expanded {
        text,
        root_len,
        modules,
    })
}

struct Done {
    path: PathBuf,
    name: String,
    alias: Option<String>,
    used_at: Span,
    source: String,
}

struct Walk<'a> {
    root_dir: &'a Path,
    packages: Option<PathBuf>,
    /// The modules being read, to catch a module that uses itself.
    stack: Vec<PathBuf>,
    /// Each module read, dependencies before the modules that use them.
    done: Vec<Done>,
}

impl Walk<'_> {
    /// Read the `use` lines of one file and return its text with them blanked.
    /// `root_use` is the line in the top file that led here.
    fn visit(
        &mut self,
        source: &str,
        dir: &Path,
        root_use: Option<Span>,
    ) -> Result<String, Diagnostic> {
        let mut out = String::with_capacity(source.len());
        let mut offset = 0;
        for line in source.split_inclusive('\n') {
            let span = Span::new(offset, offset + line.trim_end().len());
            offset += line.len();
            let Some(rest) = line.strip_prefix("use ") else {
                out.push_str(line);
                continue;
            };
            out.extend(line.chars().map(|c| if c == '\n' { c } else { ' ' }));
            let used_at = root_use.unwrap_or(span);
            let (name, alias) = parse_use(rest, span)?;
            let path = self.find(name, dir).ok_or_else(|| {
                Diagnostic::new(format!("no module `{name}`"), used_at).with_help(format!(
                    "looked for {name}.nml next to the file, in lib/ and in the installed packages"
                ))
            })?;
            if self.stack.contains(&path) {
                return Err(Diagnostic::new(
                    format!("`{name}` uses itself, directly or through another module"),
                    used_at,
                ));
            }
            if let Some(earlier) = self.done.iter().find(|d| d.path == path) {
                if earlier.alias.as_deref() != alias {
                    return Err(Diagnostic::new(
                        format!("`{name}` is used twice with different names"),
                        used_at,
                    )
                    .with_help("use a module once, or always with the same `as` name"));
                }
                continue;
            }
            let module_source = std::fs::read_to_string(&path).map_err(|e| {
                Diagnostic::new(format!("can't read {}: {e}", path.display()), used_at)
            })?;
            self.stack.push(path.clone());
            let body = self.visit(&module_source, path.parent().unwrap_or(dir), Some(used_at))?;
            self.stack.pop();
            self.done.push(Done {
                path,
                name: name.replace('/', "_"),
                alias: alias.map(str::to_string),
                used_at,
                source: body,
            });
        }
        check_private(&out, root_use)?;
        Ok(out)
    }

    fn find(&self, name: &str, dir: &Path) -> Option<PathBuf> {
        let file = format!("{name}.nml");
        [
            Some(dir.join(&file)),
            Some(self.root_dir.join("lib").join(&file)),
            self.packages.as_ref().map(|p| p.join(&file)),
        ]
        .into_iter()
        .flatten()
        .find_map(|p| p.canonicalize().ok())
    }
}

/// `rig`, `score/verse`, or either followed by `as name`.
fn parse_use(rest: &str, span: Span) -> Result<(&str, Option<&str>), Diagnostic> {
    let words: Vec<&str> = rest.split_whitespace().collect();
    let (name, alias) = match words.as_slice() {
        [name] => (*name, None),
        [name, "as", alias] => (*name, Some(*alias)),
        _ => {
            return Err(Diagnostic::new(
                format!("`use {}` isn't a module name", rest.trim()),
                span,
            )
            .with_help("write `use rig`, `use score/verse` or `use rig as r`"));
        }
    };
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'))
    {
        return Err(Diagnostic::new(
            format!("`{name}` isn't a module name"),
            span,
        ));
    }
    if let Some(alias) = alias
        && (alias.starts_with(|c: char| c.is_ascii_digit())
            || !alias.chars().all(|c| c.is_alphanumeric() || c == '_'))
    {
        return Err(Diagnostic::new(
            format!("`{alias}` isn't a name for a module"),
            span,
        ));
    }
    Ok((name, alias))
}

/// Nobody may write `rig::_name`: it is the private `_name` of `rig`.
fn check_private(source: &str, at: Option<Span>) -> Result<(), Diagnostic> {
    let Ok(tokens) = lex(source) else {
        return Ok(());
    };
    for t in tokens {
        let Tok::Ident(name) = t.tok else { continue };
        if let Some((module, private)) = name.split_once("::")
            && private.starts_with('_')
        {
            return Err(Diagnostic::new(
                format!("`{private}` is private to `{module}`"),
                at.unwrap_or(t.span),
            ));
        }
    }
    Ok(())
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
        // `x.name` calls an opcode, and `name:` is an argument unless it is a track in a scene
        let after_dot = i > 0 && tokens[i - 1].tok == Tok::Dot;
        let before_colon = tokens.get(i + 1).is_some_and(|n| n.tok == Tok::Colon);
        if (after_dot && def.kind != SymbolKind::Function)
            || (before_colon && def.kind != SymbolKind::Track)
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
        expand_with(source, dir, None)
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
        let e = expand_with("use rig\nuse score/verse\nuse acme\n", &dir, Some(packages)).unwrap();
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

    const RIG: &str = "fn shimmer(x, size: 0..1 = 0.5) { x }\ninstr tone(freq: hz) { osc(sine, freq).shimmer(size: 0.2) }\ntrack lead { instrument = tone }\nscene a { lead: riff }\nriff = [c4]\n_secret = [d4]\n";

    #[test]
    fn an_alias_renames_what_the_module_defines() {
        let dir = project(&[("rig.nml", RIG)]);
        let e = expand_here("use rig as r\nosc(saw, 1hz).r::shimmer(size: 0.1)\n", &dir).unwrap();
        let module = &e.text[e.root_len..];
        assert!(module.contains("fn r::shimmer(x, size:"), "{module}");
        assert!(module.contains(".r::shimmer(size: 0.2)"), "{module}");
        assert!(
            module.contains("instr r::tone(freq: hz)") && module.contains("instrument = r::tone"),
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
        let err = expand_here("use rig\nx.play(rig::_secret)\n", &dir)
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
