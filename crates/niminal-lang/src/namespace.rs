//! Qualify parsed definitions, preserving source spans and lexical local bindings.
use crate::ast::*;
use std::collections::{HashMap, HashSet};

fn name(item: &mut Item) -> Option<&mut Ident> {
    match item {
        Item::Instr(d) => Some(&mut d.name),
        Item::Fn(d) => Some(&mut d.name),
        Item::Track(d) => Some(&mut d.name),
        Item::Bus(d) => Some(&mut d.name),
        Item::Buffer(d) => Some(&mut d.name),
        Item::Sample(d) => Some(&mut d.name),
        Item::MidiOut(d) => Some(&mut d.name),
        Item::Control { name, .. } | Item::Const { name, .. } => Some(name),
        _ => None,
    }
}

pub(crate) fn qualify(items: &mut [Item], ranges: &[(usize, usize, String)]) {
    if ranges.is_empty() {
        return;
    }
    let mut symbols: HashMap<String, HashMap<String, String>> = HashMap::new();
    for item in items.iter_mut() {
        let function = matches!(item, Item::Fn(_));
        if let Some(id) = name(item) {
            if let Some((_, _, owner)) = ranges
                .iter()
                .find(|(a, b, _)| id.span.start >= *a && id.span.start < *b)
            {
                symbols
                    .entry(owner.clone())
                    .or_default()
                    .insert(id.name.clone(), format!("{owner}::{}", id.name));
                if function {
                    symbols.entry(owner.clone()).or_default().insert(
                        format!("@method:{}", id.name),
                        format!("{owner}::{}", id.name),
                    );
                }
            }
        }
    }
    for item in items {
        let Some(id) = name(item) else { continue };
        let Some((_, _, owner)) = ranges
            .iter()
            .find(|(a, b, _)| id.span.start >= *a && id.span.start < *b)
        else {
            continue;
        };
        let globals = &symbols[owner];
        if !id.name.starts_with("__routing_") {
            id.name = globals[&id.name].clone();
        }
        let locals = HashSet::new();
        match item {
            Item::Instr(d) => body(&mut d.params, &mut d.body, &mut d.options, globals),
            Item::Fn(d) => body(&mut d.params, &mut d.body, &mut [], globals),
            Item::Track(d) => body(&mut d.params, &mut d.body, &mut d.options, globals),
            Item::Bus(d) => {
                if let Some(stmts) = &mut d.body {
                    block(stmts, &locals, globals);
                }
            }
            Item::Buffer(d) => expr(&mut d.value, &locals, globals),
            Item::Control { value, .. } => expr(value, &locals, globals),
            Item::Const { value, .. } => expr(value, &locals, globals),
            Item::Sample(d) => {
                match &mut d.source {
                    SampleSource::Slices { sample, .. } => {
                        reference(&mut sample.name, &locals, globals)
                    }
                    SampleSource::Table(e) => expr(e, &locals, globals),
                    _ => {}
                }
                for arg in &mut d.options {
                    expr(&mut arg.value, &locals, globals);
                }
            }
            _ => {}
        }
    }
}

fn reference(n: &mut String, locals: &HashSet<String>, globals: &HashMap<String, String>) {
    if !locals.contains(n) {
        if let Some(long) = globals.get(n) {
            *n = long.clone();
        } else if let Some((base, member)) = n.split_once('.') {
            if !locals.contains(base) {
                if let Some(long) = globals.get(base) {
                    *n = format!("{long}.{member}");
                }
            }
        }
    }
}
fn body(
    params: &mut [ParamDef],
    stmts: &mut [Stmt],
    args: &mut [Arg],
    globals: &HashMap<String, String>,
) {
    let mut locals = HashSet::new();
    for p in params {
        if let Some(e) = &mut p.default {
            expr(e, &locals, globals);
        }
        locals.insert(p.name.name.clone());
    }
    for a in args {
        expr(&mut a.value, &locals, globals);
    }
    block(stmts, &locals, globals);
}
fn block(stmts: &mut [Stmt], inherited: &HashSet<String>, globals: &HashMap<String, String>) {
    let mut locals = inherited.clone();
    for s in stmts {
        // State names are visible in their own recurrence.
        if let Stmt::State { name, .. } = s {
            locals.insert(name.name.clone());
        }
        for e in s.exprs_mut() {
            expr(e, &locals, globals);
        }
        match s {
            Stmt::Bind { name, .. } | Stmt::State { name, .. } => {
                locals.insert(name.name.clone());
            }
            Stmt::Destructure { names, .. } => locals.extend(names.iter().map(|n| n.name.clone())),
            Stmt::AddAssign { name, .. } | Stmt::IndexAssign { name, .. } => {
                reference(&mut name.name, &locals, globals)
            }
            _ => {}
        }
    }
}
fn expr(e: &mut Expr, locals: &HashSet<String>, globals: &HashMap<String, String>) {
    match &mut e.kind {
        ExprKind::Name(n) => reference(n, locals, globals),
        ExprKind::Call { name, args } => {
            if args.iter().any(|a| a.receiver) {
                if let Some(long) = globals.get(&format!("@method:{}", name.name)) {
                    name.name = long.clone();
                }
            } else {
                reference(&mut name.name, locals, globals);
            }
            for arg in args {
                expr(&mut arg.value, locals, globals);
            }
        }
        ExprKind::Lambda { params, body } => {
            let mut nested = locals.clone();
            nested.extend(params.iter().map(|p| p.name.clone()));
            block(body, &nested, globals);
        }
        ExprKind::For { var, iter, body } => {
            expr(iter, locals, globals);
            let mut nested = locals.clone();
            nested.insert(var.name.clone());
            block(body, &nested, globals);
        }
        ExprKind::If {
            cond,
            then,
            otherwise,
        } => {
            expr(cond, locals, globals);
            block(then, locals, globals);
            if let Some(otherwise) = otherwise {
                block(otherwise, locals, globals);
            }
        }
        _ => e.for_each_child_mut(&mut |child| expr(child, locals, globals)),
    }
}
