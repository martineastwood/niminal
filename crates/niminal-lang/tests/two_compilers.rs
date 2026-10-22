//! A function without state is inlined into the instrument's graph; one with state is compiled to a
//! per-sample kernel. They are two compilers for one language, so each case here is run through both
//! (the second just adds an unused `state` line) and must sound the same, sample for sample.

use niminal_engine::{Voice, block_size};
use niminal_lang::compile;

fn render(source: &str, name: &str, blocks: usize) -> Result<Vec<f32>, String> {
    let program = compile(source).map_err(|errors| errors[0].message.clone())?;
    let instrument = program.instruments.iter().find(|i| i.name == name).unwrap();
    let mut voice = Voice::new(instrument.graph.clone(), 48_000.0);
    let mut out = vec![0.0; blocks * block_size()];
    for chunk in out.chunks_mut(block_size()) {
        voice.process(chunk);
    }
    Ok(out)
}

/// `body` is a function of `x` (a 110hz saw) and `y` (a 3.7hz sine), both plain numbers.
fn agree_one(body: &str) -> Result<(), String> {
    let source = |state: &str| {
        format!(
            "fn f(x, y) {{\n{state}{body}\n}}\n\
             instr a() {{ f(osc(saw, 110hz), osc(sine, 3.7hz)) }}\n"
        )
    };
    let graph = render(&source(""), "a", 40);
    let kernel = render(&source("  state unused = 0\n"), "a", 40);
    let (a, b) = match (graph, kernel) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(g), Err(k)) if g == k => return Err(format!("not valid in either: {g}")),
        (g, k) => {
            return Err(format!(
                "one compiles and the other does not: graph {:?}, kernel {:?}",
                g.err(),
                k.err()
            ));
        }
    };
    if a.iter().all(|v| *v == 0.0) {
        return Err("silent, so the case proves nothing".into());
    }
    match a.iter().zip(&b).position(|(p, q)| (p - q).abs() > 1e-5) {
        Some(i) => Err(format!(
            "differs at sample {i}: graph {}, kernel {}",
            a[i], b[i]
        )),
        None => Ok(()),
    }
}

/// Every case runs, and the failures are reported together.
fn agree_all(bodies: &[&str]) {
    let failures: Vec<String> = bodies
        .iter()
        .filter_map(|body| {
            agree_one(body)
                .err()
                .map(|e| format!("`{}`: {e}", body.replace('\n', "; ")))
        })
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn arithmetic_agrees() {
    agree_all(&[
        "x + y",
        "x - y * 0.5",
        "x * y",
        "x / (y + 1.5)",
        "x / y",
        "-x",
        "(x + 2).mod(0.77)",
        "(x + 2).mod(y + 1.5)",
        "x * 2 + 1",
        "1 / 4 + x",
    ]);
}

#[test]
fn comparison_and_selection_agree() {
    agree_all(&[
        "if x > y { x } else { y }",
        "if x < 0 { -x } else { x * 0.5 }",
        "x > 0.2",
        "x >= y",
        "x == x",
        "min(x, y)",
        "max(x, y)",
        "clamp(x, -0.3..0.4)",
        "clamp(x, (y - 1)..(y + 1))",
        "clamp(x, (y - 0.5)..0.4)",
    ]);
}

#[test]
fn math_agrees() {
    agree_all(&[
        "abs(x)",
        "sqrt(abs(x))",
        "tanh(x * 3)",
        "sin(x * 3)",
        "exp(y)",
        "floor(x * 4)",
        "pow(2, y)",
        "pow(abs(x), 2)",
        "x.abs() + y.abs()",
    ]);
}

#[test]
fn units_agree() {
    agree_all(&[
        "(440hz + 100hz * y) / 440hz",
        "(100hz + 12st) / 100hz",
        "(100hz - 12st) / 100hz",
        "((x * 6).db + 3db).num",
        "((x * 6).db - 3db).num",
        "((x * 6).db + (y * 3).db).num",
        "0.5sec / 1sec * x",
    ]);
}

#[test]
fn arrays_agree() {
    agree_all(&[
        "[x, y, 0.5].sum()",
        "[x, y].max()",
        "[x, y, x * y].map(it * 2).sum()",
        "[1, 2, 3, 4][2] * x",
        "[x, y, 0.25][1]",
        "[x, y, 0.25].len() * x",
        "[x, y] * 2 + [1, 1]",
    ]);
}

#[test]
fn statements_agree() {
    agree_all(&[
        "a = x * 2\n a + y",
        "a = x\n a = a * y\n a + 1",
        "a = 0\n for i in 0..4 { a = a + x * i }\n a",
        "a = 0\n for v in [x, y, 1] { a = a + v }\n a",
        "a = x\n if y > 0 { a = a * 2 }\n a",
        "a = x\n if y > 0 { a = a + 1 } else { a = a - 1 }\n a",
        "[a, b] = [x, y]\n a - b",
    ]);
}

#[test]
fn opcodes_and_tables_agree() {
    agree_all(&[
        "x.lpf(800hz)",
        "x.lpf(400hz + 300hz * y)",
        "osc(sine, 220hz + 50hz * y) * x",
        "[1, 0.5, 0.25][2] * x",
        "x * [1, 0.5, 0.25].sum()",
        "(0..<4).map(it * 0.25).sum() * x",
        "x.delay(1ms)",
    ]);
}
