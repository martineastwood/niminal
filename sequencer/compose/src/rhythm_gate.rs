//! Rhythmic DSP grids lowered to ordinary sample-by-sample niminal code.
use crate::ast::{Arg, Expr, ExprKind, Item, Stmt};
use crate::lexer::{Tok, lex};
use crate::parser::parse_spanned;
use niminal_lang::{Diagnostic, Span};

pub(crate) const HELPER: &str = r#"
fn __niminal_rhythm_gate(__rg_levels: [num], __rg_before: [num], __rg_after: [num],
    __rg_position: num, __rg_step: sec, __rg_width: num, __rg_attack: sec, __rg_release: sec, __rg_always_open: num) {
  __rg_index = floor(__rg_position)
  __rg_phase = __rg_position - __rg_index
  __rg_w = __rg_width.max(0).min(1)
  __rg_joined = __rg_w >= 1
  __rg_elapsed = (__rg_phase + if __rg_joined { __rg_before[__rg_index] } else { 0 }) * __rg_step
  __rg_duration = if __rg_joined { (__rg_before[__rg_index] + 1 + __rg_after[__rg_index]) * __rg_step } else { __rg_step * __rg_w }
  __rg_a = __rg_attack.max(0sec)
  __rg_r = __rg_release.max(0sec)
  __rg_scale = (__rg_duration / (__rg_a + __rg_r).max(0.000000001sec)).min(1)
  __rg_rise = if __rg_a > 0sec { (__rg_elapsed / (__rg_a * __rg_scale).max(0.000000001sec)).min(1) } else { 1 }
  __rg_fall = if __rg_r > 0sec { ((__rg_duration - __rg_elapsed) / (__rg_r * __rg_scale).max(0.000000001sec)).max(0).min(1) }
         else { if __rg_elapsed < __rg_duration { 1 } else { 0 } }
  __rg_shape = if __rg_joined and __rg_always_open { 1 } else { __rg_rise.min(__rg_fall) }
  __rg_levels[__rg_index] * __rg_shape
}
"#;

struct Grid<'a> {
    raw: &'a str,
    span: Span,
    over: Option<&'a Expr>,
    width: Option<&'a Expr>,
}

type Res<T> = Result<T, Diagnostic>;

fn grid(expr: &Expr) -> Res<Option<Grid<'_>>> {
    match &expr.kind {
        ExprKind::Grid(raw) => Ok(Some(Grid {
            raw,
            span: expr.span,
            over: None,
            width: None,
        })),
        ExprKind::Call { name, args }
            if matches!(name.name.as_str(), "over" | "width")
                && args.first().is_some_and(|a| a.receiver) =>
        {
            let Some(mut result) = grid(&args[0].value)? else {
                return Ok(None);
            };
            let [_, value] = args.as_slice() else {
                return Err(Diagnostic::new(
                    format!("a DSP grid's `{}` takes one value", name.name),
                    expr.span,
                ));
            };
            if value.name.is_some() {
                return Err(Diagnostic::new(
                    "give the grid setting without an argument name",
                    value.value.span,
                ));
            }
            let setting = if name.name == "over" {
                &mut result.over
            } else {
                &mut result.width
            };
            if setting.replace(&value.value).is_some() {
                return Err(Diagnostic::new(
                    format!("grid `{}` is given twice", name.name),
                    expr.span,
                ));
            }
            Ok(Some(result))
        }
        _ => Ok(None),
    }
}

fn text<'a>(source: &'a str, expr: &Expr) -> &'a str {
    &source[expr.span.start..expr.span.end]
}

fn nonnegative(expr: &Expr, setting: &str) -> Res<()> {
    if matches!(expr.kind, ExprKind::Neg(_))
        || matches!(expr.kind, ExprKind::Num { value, .. } if !value.is_finite() || value < 0.0)
    {
        return Err(Diagnostic::new(
            format!("gate {setting} must be nonnegative"),
            expr.span,
        ));
    }
    Ok(())
}

fn signal(
    source: &str,
    grid: Grid<'_>,
    width: Option<&Expr>,
    attack: Option<&Expr>,
    release: Option<&Expr>,
    shaped: bool,
) -> Res<String> {
    let mut levels = Vec::new();
    for c in grid.raw.chars().filter(|c| !c.is_whitespace()) {
        levels.push(match c {
            'x' => 1.0,
            'X' => 10f64.powf(3.0 / 20.0),
            'o' => 10f64.powf(-12.0 / 20.0),
            '.' => 0.0,
            _ => {
                return Err(Diagnostic::new(
                    format!("`{c}` isn't a grid step: use x, X, o or ."),
                    grid.span,
                ));
            }
        });
    }
    if levels.is_empty() {
        return Err(Diagnostic::new(
            "a DSP gate grid needs at least one step",
            grid.span,
        ));
    }
    let beats = match grid.over {
        None => super::dsp::LIVE_METER.to_string(),
        Some(expr) => match &expr.kind {
            ExprKind::Num {
                value,
                unit: Some(unit),
            } if value.is_finite() && *value > 0.0 => match unit.as_str() {
                "bar" | "bars" => format!("({value} * {})", super::dsp::LIVE_METER),
                "beat" | "beats" => value.to_string(),
                _ => {
                    return Err(Diagnostic::new(
                        "a DSP grid's `over` needs positive beats or bars",
                        expr.span,
                    ));
                }
            },
            _ => {
                return Err(Diagnostic::new(
                    "a DSP grid's `over` needs positive beats or bars",
                    expr.span,
                ));
            }
        },
    };
    let width = width.or(grid.width);
    if let Some(expr) = width {
        nonnegative(expr, "width")?;
        if let ExprKind::Num { value, unit } = &expr.kind {
            let limit = if unit.as_deref() == Some("%") {
                100.0
            } else {
                1.0
            };
            if matches!(unit.as_deref(), None | Some("%")) && *value > limit {
                return Err(Diagnostic::new(
                    "gate width must be between 0% and 100%",
                    expr.span,
                ));
            }
        }
    }
    for (expr, name) in [(attack, "attack"), (release, "release")] {
        if let Some(expr) = expr {
            nonnegative(expr, name)?;
        }
    }
    let always_open = levels.iter().all(|v| *v > 0.0);
    let n = levels.len();
    let mut before = vec![0; n];
    let mut after = vec![0; n];
    if !always_open {
        let mut run = 0;
        for i in 0..2 * n {
            if levels[i % n] > 0.0 {
                before[i % n] = run;
                run += 1;
            } else {
                run = 0;
            }
        }
        run = 0;
        for i in (0..2 * n).rev() {
            if levels[i % n] > 0.0 {
                after[i % n] = run;
                run += 1;
            } else {
                run = 0;
            }
        }
    }
    let array = |values: Vec<String>| format!("[{}]", values.join(","));
    Ok(format!(
        "__niminal_rhythm_gate({}, {}, {}, clock.beat / ({beats}) * {n}, 60sec / {} * ({beats}) / {n}, {}, {}, {}, {})",
        array(levels.iter().map(ToString::to_string).collect()),
        array(before.iter().map(ToString::to_string).collect()),
        array(after.iter().map(ToString::to_string).collect()),
        super::dsp::LIVE_BPM,
        width.map_or("1", |e| text(source, e)),
        attack.map_or(if shaped { "2ms" } else { "0sec" }, |e| text(source, e)),
        release.map_or(if shaped { "8ms" } else { "0sec" }, |e| text(source, e)),
        u8::from(always_open)
    ))
}

fn gate<'a>(source: &str, expr: &'a Expr, args: &'a [Arg]) -> Res<String> {
    let Some(pattern) = args.get(1).filter(|a| a.name.is_none()) else {
        return Err(Diagnostic::new(
            "`audio.gate` needs a grid as its first argument",
            expr.span,
        ));
    };
    let pattern = grid(&pattern.value)?.ok_or_else(|| {
        Diagnostic::new(
            "`audio.gate` needs an inline grid; multiply audio by a named gate signal instead",
            pattern.value.span,
        )
    })?;
    let mut width = None;
    let mut attack = None;
    let mut release = None;
    for arg in &args[2..] {
        let setting = match arg.name.as_ref().map(|n| n.name.as_str()) {
            Some("width") => &mut width,
            Some("attack") => &mut attack,
            Some("release") => &mut release,
            _ => {
                return Err(Diagnostic::new(
                    "gate settings are `width`, `attack` and `release`",
                    arg.value.span,
                ));
            }
        };
        if setting.replace(&arg.value).is_some() {
            return Err(Diagnostic::new(
                "gate setting is given twice",
                arg.value.span,
            ));
        }
    }
    let shape = signal(source, pattern, width, attack, release, true)?;
    let audio = &args[0].value;
    let mut edits = Vec::new();
    visit(source, audio, &mut edits)?;
    let mut input = text(source, audio).to_string();
    for (span, replacement) in edits.into_iter().rev() {
        input.replace_range(
            span.start - audio.span.start..span.end - audio.span.start,
            &replacement,
        );
    }
    Ok(format!("(({input}) * ({shape}))"))
}

fn visit(source: &str, expr: &Expr, edits: &mut Vec<(Span, String)>) -> Res<()> {
    if let ExprKind::Call { name, args } = &expr.kind
        && name.name == "gate"
        && args.first().is_some_and(|a| a.receiver)
    {
        edits.push((expr.span, gate(source, expr, args)?));
        return Ok(());
    }
    if let Some(grid) = grid(expr)? {
        edits.push((expr.span, signal(source, grid, None, None, None, false)?));
        return Ok(());
    }
    let mut result = Ok(());
    expr.for_each_child(&mut |child| {
        if result.is_ok() {
            result = visit(source, child, edits);
        }
    });
    result
}

/// Only DSP definitions are passed here; score grids keep their event semantics.
pub(crate) fn lower(source: &str) -> Res<Option<String>> {
    if !lex(source)?.iter().any(|t| matches!(t.tok, Tok::Grid(_))) {
        return Ok(None);
    }
    let items = parse_spanned(source)?;
    let mut edits = Vec::new();
    for (item, _) in &items {
        let body: &[Stmt] = match item {
            Item::Instr(d) => &d.body,
            Item::Fn(d) => &d.body,
            Item::Fx(d) => &d.body,
            _ => &[],
        };
        for stmt in body {
            for expr in stmt.exprs() {
                visit(source, expr, &mut edits)?;
            }
        }
        match item {
            Item::Const { value, .. } | Item::Control { value, .. } => {
                visit(source, value, &mut edits)?
            }
            _ => {}
        }
    }
    if edits.is_empty() {
        return Ok(None);
    }
    edits.sort_by_key(|(span, _)| span.start);
    let mut result = source.to_string();
    for (span, replacement) in edits.into_iter().rev() {
        result.replace_range(span.start..span.end, &replacement);
    }
    Ok(Some(result))
}
