//! `niminal 0.1` on a line of its own says which versions of the language a file is written for. A bare
//! version means that one or newer; several conditions joined by commas narrow it (`niminal >=0.1, <0.3`).
//! A file without the line is run as it is.
//!
//! The line is checked and then blanked, so the spans of everything after it stay where they were.

use std::borrow::Cow;

use crate::diag::{Diagnostic, Span};

/// The version of the language this build understands: `major.minor`, raised when a change would break a
/// file written for the one before.
pub const LANGUAGE_VERSION: (u32, u32) = (0, 1);

/// `src` with its `niminal <requirement>` lines checked and blanked out.
pub fn check(src: &str) -> Result<Cow<'_, str>, Diagnostic> {
    let mut out: Option<String> = None;
    let mut offset = 0;
    for line in src.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        let indent = text.len() - text.trim_start().len();
        if let Some(requirement) = requirement_of(&text[indent..]) {
            let at = offset + indent + (text[indent..].len() - requirement.len());
            let span = Span::new(at, at + requirement.trim_end().len());
            satisfied(requirement, span)?;
            let blanked = out.get_or_insert_with(|| src.to_string());
            blanked.replace_range(
                offset + indent..offset + text.len(),
                &" ".repeat(text.len() - indent),
            );
        }
        offset += line.len();
    }
    Ok(out.map_or(Cow::Borrowed(src), Cow::Owned))
}

/// What follows `niminal` on a line that is a version line: the word, then something that starts like a
/// version or a comparison, so a name that happens to be `niminal` is left alone.
fn requirement_of(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("niminal")?;
    let spaced = rest.trim_start();
    let op_end = spaced
        .find(|c: char| !"<>=~!".contains(c))
        .unwrap_or(spaced.len());
    // `niminal = 3` is an ordinary assignment, not a requirement
    let starts_like_one = spaced[op_end..]
        .trim_start()
        .starts_with(|c: char| c.is_ascii_digit())
        && &spaced[..op_end] != "=";
    (spaced.len() < rest.len() && starts_like_one).then_some(spaced)
}

fn satisfied(requirement: &str, span: Span) -> Result<(), Diagnostic> {
    // a trailing comment is not part of the requirement
    let text = requirement.split("//").next().unwrap_or("").trim();
    for clause in text.split(',') {
        let clause = clause.trim();
        let bad = || {
            Diagnostic::new(format!("`{clause}` is not a version requirement"), span).with_help(
                "write `niminal 0.1` (that version or newer) or `niminal >=0.1, <0.3`; versions are `major.minor`",
            )
        };
        let split = clause.find(|c: char| c.is_ascii_digit()).ok_or_else(bad)?;
        let (op, version) = clause.split_at(split);
        let (major, minor) = version.trim().split_once('.').ok_or_else(bad)?;
        let wanted = (
            major.parse::<u32>().map_err(|_| bad())?,
            minor.parse::<u32>().map_err(|_| bad())?,
        );
        let have = LANGUAGE_VERSION;
        let ok = match op.trim() {
            "" | ">=" => have >= wanted,
            ">" => have > wanted,
            "<=" => have <= wanted,
            "<" => have < wanted,
            "==" => have == wanted,
            _ => return Err(bad()),
        };
        if !ok {
            let (have_major, have_minor) = have;
            return Err(Diagnostic::new(
                format!(
                    "this file is for niminal {text}, and this is niminal {have_major}.{have_minor}"
                ),
                span,
            )
            .with_help(if have < wanted {
                "update niminal, or lower the version at the top of the file"
            } else {
                "the language has changed since this file was written; update the file, or widen the version at the top"
            }));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> Result<String, String> {
        check(src).map(Cow::into_owned).map_err(|d| d.message)
    }

    #[test]
    fn a_file_without_the_line_is_untouched() {
        assert_eq!(run("tempo 120bpm\n").unwrap(), "tempo 120bpm\n");
    }

    #[test]
    fn a_bare_version_means_that_one_or_newer() {
        assert!(run("niminal 0.1\nx\n").is_ok());
        assert!(run("niminal 0.0\nx\n").is_ok());
        assert!(run("niminal 0.2\nx\n").is_err());
        assert!(run("niminal 1.0\nx\n").is_err());
    }

    #[test]
    fn ranges_narrow_it() {
        assert!(run("niminal >=0.1, <0.3\n").is_ok());
        assert!(run("niminal >0.1\n").is_err());
        assert!(run("niminal <=0.1\n").is_ok());
        assert!(run("niminal <0.1\n").is_err());
        assert!(run("niminal ==0.1\n").is_ok());
        assert!(run("niminal >=0.0, <0.1\n").is_err());
    }

    #[test]
    fn the_line_is_blanked_so_spans_do_not_move() {
        let out = run("// hi\nniminal >=0.1 // fine\ntempo 120bpm\n").unwrap();
        assert_eq!(
            out.len(),
            "// hi\nniminal >=0.1 // fine\ntempo 120bpm\n".len()
        );
        assert_eq!(out, "// hi\n                     \ntempo 120bpm\n");
    }

    #[test]
    fn a_name_that_is_niminal_is_left_alone() {
        assert_eq!(run("niminal = 3\n").unwrap(), "niminal = 3\n");
        assert_eq!(run("niminal_x 1\n").unwrap(), "niminal_x 1\n");
    }

    #[test]
    fn nonsense_is_reported() {
        for bad in ["niminal 0\n", "niminal ~=0.1\n", "niminal 0.1.2\n"] {
            assert!(
                run(bad).unwrap_err().contains("not a version requirement"),
                "{bad}"
            );
        }
    }
}
