//! Document formatting. It only fixes what carries no meaning: indentation by bracket depth, one
//! space either side of `=` and `+=`, trailing whitespace, runs of blank lines and the final
//! newline. Line breaks, other spacing, grids and patterns are left as written, so a
//! hand-aligned grid stays aligned.

/// `text` with every line indented two spaces per open bracket, and a `.method` line that
/// continues the one above one level deeper.
pub fn format(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut depth: i32 = 0;
    // `[` and `(` still open from earlier lines, and how far those lines moved
    let (mut open, mut shift) = (0, 0);
    let mut blank = false;
    for raw in text.lines() {
        let line = &spaced_assignments(raw.trim());
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if blank {
            out.push(String::new());
            blank = false;
        }
        if open > 0 {
            // the inside of a multi-line pattern or call: keep its own columns, moved with its head
            let lead = raw.len() - raw.trim_start().len();
            let indent = (lead as i32 + shift).max(0) as usize;
            out.push(format!("{:indent$}{line}", ""));
            let (net, brackets) = net_depth(line);
            depth = (depth + net).max(0);
            open = (open + brackets).max(0);
            continue;
        }
        let closers = line
            .chars()
            .take_while(|c| matches!(c, '}' | ']' | ')'))
            .count() as i32;
        let level = (depth - closers).max(0);
        let continuation = line.starts_with('.')
            && line[1..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
        let indent = 2 * (level + continuation as i32) as usize;
        out.push(format!("{:indent$}{line}", ""));
        let (net, brackets) = net_depth(line);
        depth = (depth + net).max(0);
        open = brackets.max(0);
        shift = indent as i32 - (raw.len() - raw.trim_start().len()) as i32;
    }
    if out.is_empty() {
        return String::new();
    }
    out.join("\n") + "\n"
}

/// `line` with exactly one space either side of each `=` and `+=`, leaving `==`, `<=`, `>=`,
/// `!=`, strings and comments alone.
fn spaced_assignments(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut in_string = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if in_string {
            in_string = !(c == '"');
            out.push(c);
            if c == '\\' {
                out.extend(next);
                i += 1;
                in_string = true;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if c == '/' && next == Some('/') {
            out.extend(&chars[i..]);
            break;
        } else if let Some(width) = assignment_width(&chars, i, &out) {
            let rest = chars[i + width..].iter().collect::<String>();
            out.truncate(out.trim_end().len());
            if !out.is_empty() {
                out.push(' ');
            }
            out.extend(&chars[i..i + width]);
            if !rest.trim().is_empty() {
                out.push(' ');
            }
            i += width;
            while chars.get(i) == Some(&' ') {
                i += 1;
            }
            continue;
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// The length of the `=` or `+=` starting at `i`, if that is an assignment rather than part of
/// `==`, `<=`, `>=`, `!=` or `->`.
fn assignment_width(chars: &[char], i: usize, before: &str) -> Option<usize> {
    let at = |j: usize| chars.get(j).copied();
    match (chars[i], at(i + 1), at(i + 2)) {
        ('+', Some('='), after) if after != Some('=') => Some(2),
        ('=', after, _) if after != Some('=') && !before.ends_with(['<', '>', '!', '=', '+']) => {
            Some(1)
        }
        _ => None,
    }
}

/// Open brackets minus close brackets on a line, all kinds and then just `[` and `(`, ignoring
/// strings and `//` comments.
fn net_depth(line: &str) -> (i32, i32) {
    let (mut net, mut round) = (0, 0);
    let mut in_string = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if in_string => {
                chars.next();
            }
            '"' => in_string = !in_string,
            '/' if !in_string && chars.peek() == Some(&'/') => break,
            '[' | '(' if !in_string => {
                net += 1;
                round += 1;
            }
            ']' | ')' if !in_string => {
                net -= 1;
                round -= 1;
            }
            '{' if !in_string => net += 1,
            '}' if !in_string => net -= 1,
            _ => {}
        }
    }
    (net, round)
}

#[cfg(test)]
mod tests {
    use super::format;

    #[test]
    fn indents_by_depth_and_tidies_blank_lines() {
        let messy = "\n\ninstr a(x) {\n      y = x   \n  if y > 0 {\nout = y\n}\n\n\n\n  out = y\n}\nclip c { instrument: a }\n\n\n";
        assert_eq!(
            format(messy),
            "instr a(x) {\n  y = x\n  if y > 0 {\n    out = y\n  }\n\n  out = y\n}\nclip c { instrument: a }\n"
        );
    }

    #[test]
    fn method_lines_continue_one_level_in_and_brackets_in_comments_and_strings_do_not_count() {
        let src = "fn f(x) {\n// a { in a comment\nx = sample(\"a{.wav\")\n.lpf(cutoff: 2khz)\n.gain(1)\n}\n";
        assert_eq!(
            format(src),
            "fn f(x) {\n  // a { in a comment\n  x = sample(\"a{.wav\")\n    .lpf(cutoff: 2khz)\n    .gain(1)\n}\n"
        );
    }

    #[test]
    fn assignments_get_one_space_either_side_and_comparisons_are_left_alone() {
        let src = "fx f {\n  space     +=  out.gain(-16db)\n  x=1\n  y   =   a == b\n  z = a <= b // keep   =   this\n  s = sample(\"a  =  b\")\n}\ninstr i(freq: hz   =   c4) { out = freq }\n";
        assert_eq!(
            format(src),
            "fx f {\n  space += out.gain(-16db)\n  x = 1\n  y = a == b\n  z = a <= b // keep   =   this\n  s = sample(\"a  =  b\")\n}\ninstr i(freq: hz = c4) { out = freq }\n"
        );
    }

    #[test]
    fn a_multi_line_pattern_keeps_its_columns() {
        let src = "clip c {\n    notes: [a1   a1\n            ~    a2]\n  x: 1\n}\n";
        assert_eq!(
            format(src),
            "clip c {\n  notes: [a1   a1\n          ~    a2]\n  x: 1\n}\n"
        );
    }

    #[test]
    fn grid_rows_keep_their_own_spacing_and_formatting_twice_changes_nothing() {
        let src = "hits = grid {\n    kick:  x... x...\n  snare:  .... x...\n}\n";
        let once = format(src);
        assert_eq!(
            once,
            "hits = grid {\n  kick:  x... x...\n  snare:  .... x...\n}\n"
        );
        assert_eq!(format(&once), once);
    }
}
