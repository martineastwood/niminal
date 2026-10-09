use std::fmt::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    pub fn to(self, other: Span) -> Span {
        Span { start: self.start, end: other.end }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub message: String,
    pub span: Span,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        Diagnostic { message: message.into(), span, help: None }
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Plain-text rendering with the offending line and a caret underline.
    pub fn render(&self, file: &str, source: &str) -> String {
        let start = self.span.start.min(source.len());
        let line_start = source[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = source[start..].find('\n').map_or(source.len(), |i| start + i);
        let line = &source[line_start..line_end];
        let line_no = source[..line_start].matches('\n').count() + 1;
        let col = source[line_start..start].chars().count();
        let width = source[start..self.span.end.clamp(start, line_end)].chars().count().max(1);

        let gutter = line_no.to_string();
        let pad = " ".repeat(gutter.len());
        let mut out = format!("error: {}\n{pad}--> {file}:{line_no}:{}\n{pad} |\n", self.message, col + 1);
        let _ = writeln!(out, "{gutter} | {line}");
        let _ = writeln!(out, "{pad} | {}{}", " ".repeat(col), "^".repeat(width));
        if let Some(help) = &self.help {
            let _ = writeln!(out, "{pad} = help: {help}");
        }
        out
    }
}

/// The closest of `candidates` to `name`, if any is near enough to be a typo.
pub fn closest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let limit = (name.chars().count() / 3).max(1);
    candidates
        .into_iter()
        .map(|c| (edit_distance(name, c), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Edit distance where swapping two neighbouring letters counts as one edit.
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_with_caret_and_help() {
        let src = "instr a() {\n  lpf(cutoff: 800)\n}\n";
        let at = src.find("800").unwrap();
        let d = Diagnostic::new("bad", Span::new(at, at + 3)).with_help("did you mean `800hz`?");
        let r = d.render("x.nml", src);
        assert_eq!(
            r,
            "error: bad\n --> x.nml:2:15\n  |\n2 |   lpf(cutoff: 800)\n  |               ^^^\n  = help: did you mean `800hz`?\n"
        );
    }

    #[test]
    fn suggests_near_misses_only() {
        assert_eq!(closest("cuttoff", ["cutoff", "res"]), Some("cutoff"));
        assert_eq!(closest("lfp", ["lpf", "hpf"]), Some("lpf"));
        assert_eq!(closest("banana", ["cutoff", "res"]), None);
    }
}
