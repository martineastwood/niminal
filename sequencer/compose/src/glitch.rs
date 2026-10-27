//! `glitch { stutter(length: 1/4beat): .... 234. }`: glitches drawn a step at a time, as the lanes of an effect.
//!
//! Each row is one glitch of the effect that owns the lane. A step is `.` (off), `x` (on) or a digit
//! that divides the row's first setting, so `234` is a roll that gets faster. The row `stutter` writes the
//! effect's `stutter_hold` and, for each setting in its brackets, `stutter_<setting>`.

use niminal_lang::{Diagnostic, Span};
use niminal_pattern::{Pattern, parse_with};
use niminal_score::Value;

use crate::performance::{Mutation, Step};

type Res<T> = Result<T, Diagnostic>;

/// The lanes of a clip: a name, and a pattern of the values it takes.
type Lanes = Vec<(String, Pattern<Step>)>;

/// The lanes the rows in `raw` (the text between the braces, which ends just before `span.end`) give `fx`, and
/// whether `options` (the text in `glitch(...)`) asks them to mutate.
pub(crate) fn lanes(fx: &str, options: &str, raw: &str, span: Span) -> Res<(Lanes, Option<Mutation>)> {
    let mutation = mutation(options).map_err(|message| {
        Diagnostic::new(message, span).with_help("use `glitch(mutate) { ... }` for a new variation each time the clip starts, or `glitch(seed: 7) { ... }` for the same one every time")
    })?;
    let base = span.end - 1 - raw.len();
    let mut out = Vec::new();
    let mut steps: Option<usize> = None;
    let mut line_start = 0;
    for line in raw.split('\n') {
        let at = base + line_start;
        line_start += line.len() + 1;
        let code = line.split("//").next().unwrap_or("");
        if code.trim().is_empty() {
            continue;
        }
        let line_span = Span::new(at, at + line.len());
        let (lanes, count) = row(fx, code).map_err(|message| Diagnostic::new(message, line_span))?;
        // a drawn row has a step count to match; a bracketed one is as long as a cycle, however it is cut
        if let Some(count) = count {
            match steps {
                Some(first) if first != count => {
                    return Err(Diagnostic::new(
                        format!("this row has {count} steps, but the one above has {first}"),
                        line_span,
                    )
                    .with_help("rows play together, so they need the same number of steps"));
                }
                _ => steps = Some(count),
            }
        }
        out.extend(lanes);
    }
    if out.is_empty() {
        return Err(Diagnostic::new("a glitch block needs at least one row", span)
            .with_help("for example `glitch { stutter(length: 1/4beat): .... .... .... 234. }`"));
    }
    Ok((out, mutation))
}

/// `mutate` or `seed: 7`, as written in `glitch(...)`.
fn mutation(options: &str) -> Result<Option<Mutation>, String> {
    match options.trim() {
        "" => Ok(None),
        "mutate" => Ok(Some(Mutation::Fresh)),
        other => other
            .strip_prefix("seed")
            .and_then(|rest| rest.trim_start().strip_prefix(':'))
            .and_then(|n| n.trim().parse::<u64>().ok())
            .map(|seed| Some(Mutation::Seed(seed)))
            .ok_or_else(|| format!("`glitch({other})` isn't an option: it takes `mutate` or `seed: 7`")),
    }
}

/// One row, `name(setting: value, ...): steps`, as its lanes.
fn row(fx: &str, code: &str) -> Result<(Lanes, Option<usize>), String> {
    let code = code.trim();
    let name_end = code
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(code.len());
    let name = &code[..name_end];
    if name.is_empty() {
        return Err("a glitch row starts with the name of a glitch, such as `stutter`".into());
    }
    let mut rest = code[name_end..].trim_start();
    let mut settings: Vec<(&str, Value)> = Vec::new();
    if let Some(inner) = rest.strip_prefix('(') {
        let close = inner
            .find(')')
            .ok_or_else(|| "this `(` has no matching `)`".to_string())?;
        for setting in inner[..close].split(',').filter(|s| !s.trim().is_empty()) {
            let (key, value) = setting
                .split_once(':')
                .ok_or_else(|| format!("a setting is `name: value`, not `{}`", setting.trim()))?;
            let value = value.trim().parse::<Value>().map_err(|e| e.to_string())?;
            settings.push((key.trim(), value));
        }
        rest = inner[close + 1..].trim_start();
    }
    let steps = rest
        .strip_prefix(':')
        .ok_or_else(|| format!("a glitch row is `{name}: x...` or `{name}(setting: value): x...`"))?;

    // `[x ~ 2 ~]?` is mini-notation: the usual operators work and `~` is off
    if let Some(inner) = steps.trim().strip_prefix('[') {
        let inner = inner
            .strip_suffix(']')
            .ok_or_else(|| "this `[` has no matching `]`".to_string())?;
        return mini(fx, name, &settings, inner).map(|lanes| (lanes, None));
    }

    let mut hold = Vec::new();
    let mut first = Vec::new();
    for c in steps.chars().filter(|c| !c.is_whitespace()) {
        let divisor = match c {
            '.' => None,
            'x' | 'X' => Some(1),
            '1'..='9' => Some(c.to_digit(10).unwrap_or(1)),
            other => {
                return Err(format!("`{other}` isn't a glitch step: use `.` for off, `x` for on, or a digit to divide the first setting"));
            }
        };
        hold.push(Pattern::pure(Step::from(Value::Num(f64::from(divisor.is_some())))));
        if let Some((_, value)) = settings.first() {
            // an off step says nothing, so the setting stays where the last hold left it
            first.push(match divisor {
                None => Pattern::silence(),
                Some(1) => Pattern::pure(Step::from(*value)),
                Some(n) => Pattern::pure(Step::from(divide(*value, f64::from(n))?)),
            });
        } else if divisor.is_some_and(|n| n > 1) {
            return Err(format!("a digit divides the row's first setting, so `{name}` needs one, such as `{name}(length: 1/4beat)`"));
        }
    }
    if hold.is_empty() {
        return Err(format!("`{name}` has no steps"));
    }
    let count = hold.len();
    let mut lanes = vec![(format!("{fx}::{name}_hold"), Pattern::fastcat(hold))];
    for (i, (key, value)) in settings.iter().enumerate() {
        let pattern = if i == 0 { Pattern::fastcat(std::mem::take(&mut first)) } else { Pattern::pure(Step::from(*value)) };
        lanes.push((format!("{fx}::{name}_{key}"), pattern));
    }
    Ok((lanes, Some(count)))
}

/// A row written in mini-notation, `[x ~ 2 ~]`: `x` or a digit is on, `~` is off, and the usual
/// operators (`x*2`, `x?`, `<x 2>`, `x(3,8)`) shape it.
fn mini(fx: &str, name: &str, settings: &[(&str, Value)], inner: &str) -> Result<Lanes, String> {
    let first = settings.first().map(|(_, value)| *value);
    let on = parse_with(inner, |atom, _| match atom {
        "x" | "X" => Ok(1u32),
        digit if matches!(digit.as_bytes(), [b'1'..=b'9']) => {
            let n = u32::from(digit.as_bytes()[0] - b'0');
            match first {
                _ if n == 1 => Ok(1),
                Some(value) => divide(value, f64::from(n)).map(|_| n),
                None => Err(format!("a digit divides the row's first setting, so `{name}` needs one, such as `{name}(length: 1/4beat)`")),
            }
        }
        other => Err(format!("`{other}` isn't a glitch step: use `~` for off, `x` for on, or a digit to divide the first setting")),
    })
    .map_err(|e| e.message)?;
    // a dropped or resting step makes no event, so the gaps are filled with an explicit off
    let hold_on = on.map(|_| Step::from(Value::Num(1.0)));
    let off = hold_on.invert(Step::from(Value::Num(0.0)));
    let mut lanes = vec![(format!("{fx}::{name}_hold"), Pattern::stack(vec![hold_on, off]))];
    for (i, (key, value)) in settings.iter().enumerate() {
        let value = *value;
        let pattern = if i == 0 {
            on.map(move |n| Step::from(divide(value, f64::from(n)).unwrap_or(value)))
        } else {
            Pattern::pure(Step::from(value))
        };
        lanes.push((format!("{fx}::{name}_{key}"), pattern));
    }
    Ok(lanes)
}

/// `value` split into `n` equal parts, for the quantities that can be.
fn divide(value: Value, n: f64) -> Result<Value, String> {
    Ok(match value {
        Value::Num(x) => Value::Num(x / n),
        Value::Hz(x) => Value::Hz(x / n),
        Value::Seconds(x) => Value::Seconds(x / n),
        Value::Beats(x) => Value::Beats(x / n),
        Value::Bars(x) => Value::Bars(x / n),
        Value::Semitones(x) => Value::Semitones(x / n),
        Value::Degrees(x) => Value::Degrees(x / n),
        other => return Err(format!("a digit can't divide `{other}`")),
    })
}
