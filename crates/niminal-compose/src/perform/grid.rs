//! Parsing step and grid rows.

use super::*;

/// Where in the source a problem inside a pattern's text is.
/// What a step character is worth: `.` is 0 and `x` is 1, with `X` an accent (+3db) and `o` a ghost (-12db) as in a
/// drum grid.
pub(super) fn step_level(c: char) -> f64 {
    match c {
        'x' => 1.0,
        'X' => 10f64.powf(3.0 / 20.0),
        'o' => 10f64.powf(-12.0 / 20.0),
        _ => 0.0,
    }
}

/// A row of steps as a lane. Drawn plain, each character is a step; the steps share one cycle, which the clip
/// stretches over its length like any lane. With mini-notation in it (`...<x X .>`, `x?`, `x(3,8)`) the usual
/// operators shape the steps, and a step that is off or dropped is an explicit 0, so the lane never keeps the last
/// level.
pub(super) fn steps(raw: &str) -> Result<Pattern<Step>, String> {
    if raw
        .chars()
        .all(|c| matches!(c, '.' | 'x' | 'X' | 'o' | ' ' | '\t'))
    {
        return Ok(Pattern::fastcat(
            raw.chars()
                .filter(|c| !c.is_whitespace())
                .map(|c| Pattern::pure(Step::from(Value::Num(step_level(c)))))
                .collect(),
        ));
    }
    mini_steps(raw)
}

/// `raw` as mini-notation whose atoms are the step characters, `.` and `~` being rests.
pub(super) fn mini_steps(raw: &str) -> Result<Pattern<Step>, String> {
    // each step character is a step of its own, as in a drawn row, so give it room
    let chars: Vec<char> = raw.chars().collect();
    let is_step = |c: char| matches!(c, '.' | 'x' | 'X' | 'o');
    let mut spaced = String::new();
    for (i, &c) in chars.iter().enumerate() {
        spaced.push(if c == '.' { '~' } else { c });
        let ends_a_step =
            is_step(c) || c.is_ascii_digit() || matches!(c, '>' | ']' | ')' | '?' | '!');
        if ends_a_step
            && chars
                .get(i + 1)
                .is_some_and(|&next| is_step(next) || matches!(next, '<' | '['))
        {
            spaced.push(' ');
        }
    }
    let on = parse_with(&spaced, |atom, _| match atom {
        "x" | "X" | "o" => Ok(step_level(atom.chars().next().unwrap_or('x'))),
        other => Err(format!(
            "`{other}` isn't a step: use `.` or `~` for off, and `x`, `X` or `o` for on"
        )),
    })
    .map_err(|e| e.message)?;
    // a rest or a dropped step makes no event, so the gaps are filled with an explicit off
    let on = on.map(|level| Step::from(Value::Num(level)));
    let off = on.invert(Step::from(Value::Num(0.0)));
    Ok(Pattern::stack(vec![on, off]))
}

pub(super) fn at_offset(message: String, token: Span, offset: usize) -> Diagnostic {
    // the token's text starts after its opening bracket
    let at = token.start + 1 + offset;
    Diagnostic::new(message, Span::new(at, (at + 1).min(token.end)))
}

/// One row of `grid { ... }`: a sample or clip name, optional note controls, and a body.
pub(super) struct GridRow<'a> {
    pub(super) name: &'a str,
    pub(super) name_offset: usize,
    pub(super) gain: Option<f64>,
    pub(super) pan: Option<f64>,
    pub(super) begin: Option<f64>,
    pub(super) pitch: Option<f64>,
    /// True when the body is mini-notation in brackets.
    pub(super) mini: bool,
    pub(super) body: &'a str,
    pub(super) body_offset: usize,
}

/// `kick: x...`, `snare(pan: 10deg): x...`, or `hat: [x*2 x]`.
pub(super) fn parse_grid_row(code: &str) -> Result<GridRow<'_>, (usize, String)> {
    let trim_start = code.len() - code.trim_start().len();
    let s = code.trim();
    let err = |offset: usize, message: String| (trim_start + offset, message);

    // name: letters, digits, underscores, and at most one dot for `kit.member`
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
    {
        i += 1;
    }
    if i == 0 {
        return Err(err(
            0,
            "a grid row starts with a sample or clip name".into(),
        ));
    }
    let name = &s[..i];
    let name_offset = trim_start;
    if name.starts_with('.') || name.ends_with('.') || name.matches('.').count() > 1 {
        return Err(err(0, format!("`{name}` isn't a sample or clip name")));
    }

    let mut gain = None;
    let mut pan = None;
    let mut begin = None;
    let mut pitch = None;
    if i < s.len() && s.as_bytes()[i] == b'(' {
        let open = i;
        i += 1;
        let mut depth = 1usize;
        let mut close = None;
        while i < s.len() {
            match s.as_bytes()[i] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(i);
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let Some(close) = close else {
            return Err(err(open, "this `(` has no matching `)`".into()));
        };
        let params = &s[open + 1..close];
        parse_row_params(
            params,
            open + 1,
            &mut gain,
            &mut pan,
            &mut begin,
            &mut pitch,
        )
        .map_err(|(o, m)| err(o, m))?;
        i = close + 1;
    }

    while i < s.len() && s.as_bytes()[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= s.len() || s.as_bytes()[i] != b':' {
        return Err(err(
            0,
            "a grid row is `sample: x...` or `sample: [x*2 x]`".into(),
        ));
    }
    i += 1;
    while i < s.len() && s.as_bytes()[i].is_ascii_whitespace() {
        i += 1;
    }
    let body = s[i..].trim_end();
    let body_offset = trim_start + i;
    if body.is_empty() {
        return Err(err(i, "a grid row needs hits after the `:`".into()));
    }
    if body.starts_with('[') {
        if !body.ends_with(']') {
            return Err(err(i, "this `[` has no matching `]`".into()));
        }
        // Strip the outer brackets; parse_with expects the mini-notation text.
        let inner = &body[1..body.len() - 1];
        return Ok(GridRow {
            name,
            name_offset,
            gain,
            pan,
            begin,
            pitch,
            mini: true,
            body: inner,
            body_offset: body_offset + 1,
        });
    }
    Ok(GridRow {
        name,
        name_offset,
        gain,
        pan,
        begin,
        pitch,
        mini: false,
        body,
        body_offset,
    })
}

/// `gain: -6db, pan: 10deg, pitch: -2st, begin: 25%` inside a row's `(...)`.
pub(super) fn parse_row_params(
    src: &str,
    base: usize,
    gain: &mut Option<f64>,
    pan: &mut Option<f64>,
    begin: &mut Option<f64>,
    pitch: &mut Option<f64>,
) -> Result<(), (usize, String)> {
    let err = |offset: usize, message: String| (base + offset, message);
    for part in src.split(',') {
        let part_start = part.as_ptr() as usize - src.as_ptr() as usize;
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((key, value)) = part.split_once(':') else {
            return Err(err(
                part_start,
                "a row parameter is `name: value`, such as `pan: 10deg`".into(),
            ));
        };
        let key = key.trim();
        let value = value.trim();
        let value_at = part_start + (value.as_ptr() as usize - part.as_ptr() as usize);
        // `%` is a share, not a Value unit; everything else uses the usual parse.
        let parsed = if let Some(n) = value.strip_suffix('%') {
            let n: f64 = n
                .trim()
                .parse()
                .map_err(|_| err(value_at, format!("`{value}` is not a percentage")))?;
            Value::Num(n / 100.0)
        } else {
            value
                .parse()
                .map_err(|e: niminal_score::ParseError| err(value_at, e.0))?
        };
        match key {
            "gain" => {
                let db = match parsed {
                    Value::Db(db) => db,
                    Value::Num(n) if n > 0.0 => 20.0 * n.log10(),
                    _ => {
                        return Err(err(
                            value_at,
                            "gain is a level in db, such as `-6db`".into(),
                        ));
                    }
                };
                if gain.replace(db).is_some() {
                    return Err(err(part_start, "`gain` is given more than once".into()));
                }
            }
            "pan" => {
                let Value::Degrees(d) = parsed else {
                    return Err(err(
                        value_at,
                        "pan is a direction in degrees, such as `10deg`".into(),
                    ));
                };
                if pan.replace(d).is_some() {
                    return Err(err(part_start, "`pan` is given more than once".into()));
                }
            }
            "begin" => {
                let share = match parsed {
                    Value::Num(n) if (0.0..=1.0).contains(&n) => n,
                    _ => {
                        return Err(err(
                            value_at,
                            "begin is a share of the sample, such as `25%`".into(),
                        ));
                    }
                };
                if begin.replace(share).is_some() {
                    return Err(err(part_start, "`begin` is given more than once".into()));
                }
            }
            "pitch" => {
                let Value::Semitones(st) = parsed else {
                    return Err(err(
                        value_at,
                        "pitch is an interval in semitones, such as `-2st`".into(),
                    ));
                };
                if pitch.replace(st).is_some() {
                    return Err(err(part_start, "`pitch` is given more than once".into()));
                }
            }
            other => {
                return Err(err(
                    part_start,
                    format!("`{other}` isn't a row parameter (gain, pan, begin, or pitch)"),
                ));
            }
        }
    }
    Ok(())
}
