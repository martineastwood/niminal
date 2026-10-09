//! The project a session is playing: definitions kept as source text, so that
//! a snippet can replace one of them and the whole thing be compiled again.

use niminal_lang::{Diagnostic, Span};

/// One top-level definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// `instr:pluck`, `clip:riff`, `config`... Defining a key again replaces it.
    pub key: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Project {
    pub entries: Vec<Entry>,
}

impl Project {
    /// Add or replace definitions. A replaced definition keeps its place, since
    /// later definitions may depend on it being earlier.
    pub fn apply(&mut self, delta: &[Entry]) {
        for e in delta {
            match self.entries.iter_mut().find(|x| x.key == e.key) {
                Some(existing) => existing.text.clone_from(&e.text),
                None => self.entries.push(e.clone()),
            }
        }
    }

    pub fn text_of(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|e| e.key == key).map(|e| e.text.as_str())
    }

    /// The source for compiling, plus `extra` statements (actions) after it. The
    /// `map` says which part of the source came from where, so an error can be
    /// pointed at the snippet the user sent.
    ///
    /// `from_snippet` gives the offset in the user's snippet of each definition
    /// that came from it; those are attributed to the snippet, the rest to the
    /// earlier definitions they are.
    pub fn source(&self, from_snippet: &std::collections::HashMap<String, usize>, extra: &[Piece]) -> (String, SourceMap) {
        let mut text = String::new();
        let mut map = SourceMap::default();
        for e in &self.entries {
            let origin = match from_snippet.get(&e.key) {
                Some(&offset) => Origin::Snippet { offset },
                None => Origin::Definition(e.key.clone()),
            };
            map.add(&mut text, &e.text, origin);
        }
        for p in extra {
            map.add(&mut text, &p.text, p.origin.clone());
        }
        (text, map)
    }

    /// The text of every definition that is not a track, instrument or other
    /// body, which a track's effect chain depends on: opcodes, buses, the
    /// output layout, meter and tempo.
    pub fn environment(&self) -> String {
        self.entries
            .iter()
            .filter(|e| {
                e.key.starts_with("opcode:")
                    || e.key.starts_with("bus:")
                    || matches!(e.key.as_str(), "config" | "meter" | "tempo")
            })
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A statement being added to a compile, and where it came from.
#[derive(Debug, Clone)]
pub struct Piece {
    pub text: String,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    /// An earlier, already committed definition.
    Definition(String),
    /// A statement of the snippet being evaluated, at this offset in it.
    Snippet { offset: usize },
}

#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    parts: Vec<(usize, usize, Origin)>,
}

impl SourceMap {
    fn add(&mut self, into: &mut String, text: &str, origin: Origin) {
        let start = into.len();
        into.push_str(text);
        self.parts.push((start, into.len(), origin));
        into.push('\n');
    }

    /// Where a span of the compiled source is in the user's terms.
    pub fn locate(&self, span: Span) -> Located {
        for (start, end, origin) in &self.parts {
            if span.start >= *start && span.start <= *end {
                return match origin {
                    Origin::Snippet { offset } => Located::Snippet(Span::new(
                        offset + (span.start - start),
                        offset + (span.end.min(*end) - start),
                    )),
                    Origin::Definition(key) => Located::Earlier(key.clone()),
                };
            }
        }
        Located::Earlier("the project".into())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Located {
    /// In the snippet, at this span of it.
    Snippet(Span),
    /// In a definition that was already there.
    Earlier(String),
}

/// A problem with a snippet, in terms of the snippet.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub message: String,
    pub help: Option<String>,
    /// 1-based line and column in the snippet, when it is in the snippet.
    pub line: Option<usize>,
    pub column: Option<usize>,
    /// The span in the snippet.
    pub span: Option<Span>,
    /// For a problem in an earlier definition: its key.
    pub in_definition: Option<String>,
}

impl Problem {
    pub fn plain(message: impl Into<String>) -> Problem {
        Problem { message: message.into(), help: None, line: None, column: None, span: None, in_definition: None }
    }

    pub fn from_diagnostic(d: &Diagnostic, snippet: &str, located: Located) -> Problem {
        let mut p = Problem::plain(d.message.clone());
        p.help.clone_from(&d.help);
        match located {
            Located::Snippet(span) => {
                let start = span.start.min(snippet.len());
                let before = &snippet[..start];
                p.line = Some(before.matches('\n').count() + 1);
                p.column = Some(before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1);
                p.span = Some(span);
            }
            Located::Earlier(key) => p.in_definition = Some(key),
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, text: &str) -> Entry {
        Entry { key: key.into(), text: text.into() }
    }

    #[test]
    fn redefining_replaces_in_place() {
        let mut p = Project::default();
        p.apply(&[entry("a", "A1"), entry("b", "B1"), entry("c", "C1")]);
        p.apply(&[entry("b", "B2"), entry("d", "D1")]);
        let keys: Vec<_> = p.entries.iter().map(|e| (e.key.as_str(), e.text.as_str())).collect();
        assert_eq!(keys, [("a", "A1"), ("b", "B2"), ("c", "C1"), ("d", "D1")]);
        assert_eq!(p.text_of("b"), Some("B2"));
        assert_eq!(p.text_of("zz"), None);
    }

    #[test]
    fn errors_are_located_in_the_snippet_or_in_earlier_definitions() {
        let mut p = Project::default();
        p.apply(&[entry("instr:a", "instr a() { osc(sine, 100hz) }")]);
        let snippet = "\n\nplay lead = nope";
        let extra = [Piece { text: "play lead = nope".into(), origin: Origin::Snippet { offset: 2 } }];
        let (text, map) = p.source(&std::collections::HashMap::new(), &extra);
        let at = text.find("nope").unwrap();
        let located = map.locate(Span::new(at, at + 4));
        assert_eq!(located, Located::Snippet(Span::new(2 + "play lead = ".len(), 2 + "play lead = nope".len())));

        let d = Diagnostic::new("x", Span::new(0, 1));
        assert_eq!(map.locate(Span::new(0, 5)), Located::Earlier("instr:a".into()));
        let problem = Problem::from_diagnostic(&d, snippet, Located::Snippet(Span::new(14, 18)));
        assert_eq!((problem.line, problem.column), (Some(3), Some(13)));
    }

    #[test]
    fn the_environment_is_what_effect_chains_depend_on() {
        let mut p = Project::default();
        p.apply(&[
            entry("config", "config { channels: stereo }"),
            entry("instr:a", "instr a() { 0.5 }"),
            entry("bus:s", "bus s"),
            entry("opcode:o", "opcode o(x) { x }"),
            entry("track:t", "track t { out = it }"),
        ]);
        let env = p.environment();
        assert!(env.contains("channels: stereo") && env.contains("bus s") && env.contains("opcode o"));
        assert!(!env.contains("instr a") && !env.contains("track t"));
    }
}
