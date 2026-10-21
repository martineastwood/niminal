//! Splitting source into its top-level definitions, for a session that applies a snippet to a running
//! project.

use crate::ast::*;
use crate::diag::Diagnostic;
use crate::parser::parse_spanned;

/// One top-level definition of the source.
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub text: String,
    pub offset: usize,
    /// What it defines, as `instr:name`, `node:name`, `config`...: defining a key again replaces it.
    pub key: String,
}

/// Split `source` into definitions.
pub fn analyze(source: &str) -> Result<Vec<Statement>, Diagnostic> {
    let mut out = Vec::new();
    for (item, span) in parse_spanned(source)? {
        let key = match &item {
            Item::Control { name, .. } => format!("ctl:{}", name.name),
            Item::Instr(d) => format!("instr:{}", d.name.name),
            Item::Fn(d) => format!("fn:{}", d.name.name),
            Item::Const { name, .. } => format!("const:{}", name.name),
            Item::MidiOut(d) => format!("instr:{}", d.name.name),
            Item::Sample(d) => format!(
                "{}:{}",
                if d.is_kit { "kit" } else { "sample" },
                d.name.name
            ),
            Item::Bus(d) => format!("bus:{}", d.name.name),
            Item::Buffer(d) => format!("buffer:{}", d.name.name),
            Item::Node(d) => format!("node:{}", d.name.name),
            Item::Fx(d) => format!("fx:{}", d.name.name),
            Item::Config(_) => "config".into(),
        };
        out.push(Statement {
            text: source[span.start..span.end].to_string(),
            offset: span.start,
            key,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements_are_classified_with_their_source() {
        let src =
            "\ninstr a(f: hz) { osc(sine, f) }\nctl bright = 3khz\nnode t { instrument = a }\n";
        let s = analyze(src).unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].key, "instr:a");
        assert_eq!(s[0].text, "instr a(f: hz) { osc(sine, f) }");
        assert_eq!(&src[s[0].offset..s[0].offset + 5], "instr");
        assert_eq!(s[1].key, "ctl:bright");
        assert_eq!(s[2].key, "node:t");
    }

    #[test]
    fn syntax_errors_come_back_as_diagnostics() {
        assert!(analyze("instr (").is_err());
        assert!(analyze("").unwrap().is_empty());
    }
}
