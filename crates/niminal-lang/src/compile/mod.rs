//! Program-level compilation: tempo, functions, instruments and notes.

mod nodes;
mod samples;
mod config;
mod literals;
#[cfg(test)]
mod tests;
use nodes::*;
pub(crate) use samples::*;
use config::*;
pub use literals::*;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use niminal_engine::{NodeDef, Route, execution_order};
use niminal_score::{MAX_DEGREES, Tempo, Time, Tuning, Value};

use crate::ast::*;
use crate::diag::{Diagnostic, Span, closest};
use crate::env_shortcut;
use crate::layout::Layout;
use crate::lower::{
    Datum, Member, Mixer, Names, SamplerDef, SamplerSource, check_fn, compile_chain, compile_const,
    compile_instr, compile_sampler, layout_from_expr, unknown_unit,
};
use crate::opcodes::Registry;
use crate::parser;
use crate::program::{Instrument, NodeInfo, Program};
use crate::sample::Samples;

type Res<T> = Result<T, Diagnostic>;

const DEFAULT_BPM: f64 = 120.0;

/// Settings for compiling a project.
#[derive(Clone)]
pub struct CompileOptions {
    /// Source ranges belonging to private engine namespaces. Spans remain in original source.
    pub namespaces: Vec<(usize, usize, String)>,
    pub namespace_samples: HashMap<String, Samples>,
    pub sample_rate: f32,
    pub buffer_budgets: HashMap<String, usize>,
    pub retained_buffer_bytes: HashMap<String, usize>,
    /// Defer topology-dependent compensation until the host has connected fx instances.
    pub defer_latency: bool,
    /// The output layout when the source has no `config { channels: ... }`.
    pub default_layout: Layout,
    /// Host-selected output layouts for native routing fx templates. Other nodes keep their declared destination.
    pub node_outputs: HashMap<String, Layout>,
    /// Where the files named by `sample` and `kit` are found.
    pub samples: Samples,
    /// Existing shared resources to retain when a definition keeps its name, length and rate.
    pub buffers: HashMap<String, Arc<niminal_engine::Buffer>>,
    /// Where each definition in the source was sent from (`Project::origins`), so that a note can
    /// remember where it was written in what the client sent. `None` keeps positions as they are.
    pub origins: Option<Vec<(usize, usize, u64, usize)>>,
    /// Leave out what a clip names that is not there (a grid row for a drum its kit does not have, a lane
    /// for a parameter its instrument does not have) and report it in `Composition::skipped`, instead of
    /// refusing the clip. For live hosts, where definitions arrive in any order and change under clips
    /// that are playing.
    pub lenient: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        CompileOptions {
            namespaces: Vec::new(),
            namespace_samples: HashMap::new(),
            sample_rate: 48000.,
            buffer_budgets: HashMap::new(),
            retained_buffer_bytes: HashMap::new(),
            defer_latency: false,
            default_layout: Layout::Mono,
            node_outputs: HashMap::new(),
            samples: Samples::default(),
            buffers: HashMap::new(),
            origins: None,
            lenient: false,
        }
    }
}

/// Lower fx parameters to named controls.
/// A parameter typed `division` is a length that the body sees snapped to the nearest musical division at the
/// tempo: the body's uses of it read `snap_division` of the parameter, bound to a name of its own. This runs
/// before names are qualified, so the call is qualified with the rest of the text it came from.
fn snap_divisions(items: &mut [Item]) {
    for item in items {
        let (params, body) = match item {
            Item::Instr(d) => (&d.params, &mut d.body),
            Item::Fn(d) => (&d.params, &mut d.body),
            Item::Node(d) | Item::Fx(d) => (&d.params, &mut d.body),
            _ => continue,
        };
        for param in params {
            if !matches!(&param.ty, Some(TypeSpec::Unit(t)) if t.name == "division") {
                continue;
            }
            let snapped = format!("{}__snapped", param.name.name);
            for stmt in body.iter_mut() {
                for e in stmt.exprs_mut() {
                    rename(e, &param.name.name, &snapped);
                }
            }
            let at = param.name.span;
            let read = Expr {
                kind: ExprKind::Name(param.name.name.clone()),
                span: at,
            };
            let call = Expr {
                kind: ExprKind::Call {
                    name: Ident {
                        name: "snap_division".into(),
                        span: at,
                    },
                    args: vec![Arg {
                        name: None,
                        value: read,
                        receiver: false,
                    }],
                },
                span: at,
            };
            body.insert(
                0,
                Stmt::Bind {
                    name: Ident {
                        name: snapped,
                        span: at,
                    },
                    value: call,
                },
            );
        }
    }
}

fn desugar(items: Vec<Item>) -> Vec<Item> {
    let mut out = Vec::with_capacity(items.len());
    for mut item in items {
        match &mut item {
            Item::Node(node) | Item::Fx(node) => {
                // a parameter is a control called `node::param`, and the body reads it by its short name
                for param in std::mem::take(&mut node.params) {
                    let Some(default) = param.default else {
                        continue;
                    };
                    let long = format!("{}::{}", node.name.name, param.name.name);
                    for stmt in &mut node.body {
                        for e in stmt.exprs_mut() {
                            rename(e, &param.name.name, &long);
                        }
                    }
                    out.push(Item::Control {
                        name: Ident {
                            name: long,
                            span: param.name.span,
                        },
                        value: default,
                        input: None,
                    });
                }
            }
            _ => {}
        }
        out.push(item);
    }
    out
}

fn prepare_routing(parsed: &mut Vec<Item>) -> Res<BTreeMap<String, String>> {
    // Collections own note defaults and routing. Persistent DSP belongs to fx declarations.
    let fx_names: Vec<String> = parsed
        .iter()
        .filter_map(|item| match item {
            Item::Fx(d) => Some(d.name.name.clone()),
            _ => None,
        })
        .collect();
    let mut fx_inputs = BTreeMap::new();
    for item in parsed.iter_mut() {
        match item {
            Item::Fx(d) => {
                if d.body
                    .iter()
                    .any(|s| matches!(s, Stmt::Bind { name, .. } if name.name == "instrument"))
                {
                    return Err(Diagnostic::new(
                        "an fx cannot select an instrument; select it on the clip or note",
                        d.span,
                    ));
                }
            }
            Item::Node(d) => {
                if !d.params.is_empty() {
                    return Err(Diagnostic::new(
                        "collection nodes have no DSP parameters; declare them on an fx",
                        d.span,
                    ));
                }
                for stmt in &mut d.body {
                    let Stmt::Bind { name, value } = stmt else {
                        return Err(Diagnostic::new(
                            "a collection accepts instrument, output, gain and pan; put DSP in an fx",
                            d.span,
                        ));
                    };
                    match name.name.as_str() {
                        "instrument" | "gain" | "pan" => {}
                        "output" => {
                            let ExprKind::Name(destination) = &value.kind else {
                                return Err(Diagnostic::new(
                                    "output is the name of a bus or fx",
                                    value.span,
                                ));
                            };
                            let destination = if fx_names.contains(destination) {
                                fx_inputs
                                    .entry(destination.clone())
                                    .or_insert_with(|| format!("{destination}.__input"))
                                    .clone()
                            } else {
                                destination.clone()
                            };
                            name.name = "out".into();
                            *value = Expr {
                                span: value.span,
                                kind: ExprKind::Call {
                                    name: Ident {
                                        name: "to".into(),
                                        span: name.span,
                                    },
                                    args: vec![
                                        Arg {
                                            name: None,
                                            receiver: true,
                                            value: Expr {
                                                kind: ExprKind::Name("in".into()),
                                                span: name.span,
                                            },
                                        },
                                        Arg {
                                            name: None,
                                            receiver: false,
                                            value: Expr {
                                                kind: ExprKind::Name(destination),
                                                span: name.span,
                                            },
                                        },
                                    ],
                                },
                            };
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                "a collection accepts instrument, output, gain and pan; put DSP in an fx",
                                name.span,
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    for input in fx_inputs.values() {
        parsed.push(Item::Bus(BusDecl {
            name: Ident {
                name: input.clone(),
                span: Span::default(),
            },
            layout: None,
        }));
    }
    Ok(fx_inputs)
}

/// A collection may share the name of its default instrument.
fn plays_its_namesake(node: &NodeDecl) -> bool {
    node.body.iter().any(|stmt| matches!(stmt, Stmt::Bind { name, value }
        if name.name == "instrument" && matches!(&value.kind, ExprKind::Name(n) if *n == node.name.name)))
}

/// Every use of the name `from` in `e` becomes `to`.
// inline(never): rename and for_each_child_mut recurse through each other; letting LLVM inline them
// made `codegen-units = 1` builds (release-fast) take 10+ minutes instead of seconds.
#[inline(never)]
fn rename(e: &mut Expr, from: &str, to: &str) {
    if let ExprKind::Name(name) = &mut e.kind
        && name == from
    {
        *name = to.to_string();
    }
    e.for_each_child_mut(&mut |c| rename(c, from, to));
}

pub fn compile(source: &str) -> Result<Program, Vec<Diagnostic>> {
    compile_with(source, &CompileOptions::default())
}

/// The instruments compiled so far, and the names of ones that failed (so a second use of the name is still reported).
#[derive(Default)]
struct Defined<'a> {
    instruments: Vec<Instrument>,
    failed: Vec<&'a str>,
}

impl<'a> Defined<'a> {
    fn add(
        &mut self,
        name: &'a Ident,
        errors: &mut Vec<Diagnostic>,
        compile: impl FnOnce() -> Result<Instrument, Diagnostic>,
    ) {
        let name_text = name.name.as_str();
        if self.instruments.iter().any(|i| i.name == name_text) || self.failed.contains(&name_text)
        {
            errors.push(Diagnostic::new(
                format!("`{name_text}` is defined twice"),
                name.span,
            ));
            return;
        }
        match compile() {
            Ok(i) => self.instruments.push(i),
            Err(d) => {
                errors.push(d);
                self.failed.push(name_text);
            }
        }
    }
}

pub fn compile_with(source: &str, options: &CompileOptions) -> Result<Program, Vec<Diagnostic>> {
    // a server has done this for its client already, with that client's tempo
    let source = &crate::lower_clock(source, crate::ClockState::default()).map_err(|d| vec![d])?;
    crate::validate_dsp(source).map_err(|d| vec![d])?;
    let mut parsed = parser::parse(source).map_err(|d| vec![d])?;
    snap_divisions(&mut parsed);
    crate::namespace::qualify(&mut parsed, &options.namespaces);
    let mut namespace_tempos = HashMap::new();
    let mut configs_by_owner: HashMap<&str, Vec<Item>> = HashMap::new();
    for (start, end, owner) in &options.namespaces {
        configs_by_owner.entry(owner).or_default().extend(parsed.iter().filter(|item|matches!(item,Item::Config(c) if c.span.start>=*start && c.span.start<*end)).cloned());
    }
    for (owner, configs) in configs_by_owner {
        let tuning = tuning_of(&configs).map_err(|d| vec![d])?;
        namespace_tempos.insert(
            owner,
            Tempo {
                tuning,
                ..Tempo::new(DEFAULT_BPM)
            },
        );
    }
    let owned_tempo = |name: &str, fallback: Tempo| {
        name.split_once("::")
            .and_then(|(owner, _)| namespace_tempos.get(owner))
            .copied()
            .unwrap_or(fallback)
    };
    if !options.namespaces.is_empty() {
        for item in &parsed {
            if let Item::Config(config) = item
                && options
                    .namespaces
                    .iter()
                    .any(|(start, end, _)| config.span.start >= *start && config.span.start < *end)
            {
                for (key, value) in &config.entries {
                    if key.name == "channels"
                        && layout_from_expr(value).map_err(|d| vec![d])? != options.default_layout
                    {
                        return Err(vec![Diagnostic::new(
                            "client output layout must match the running engine",
                            value.span,
                        )]);
                    }
                }
            }
        }
        parsed.retain(|item|!matches!(item,Item::Config(config) if options.namespaces.iter().any(|(start,end,_)|config.span.start>=*start&&config.span.start<*end)));
    }
    let fx_inputs = prepare_routing(&mut parsed).map_err(|d| vec![d])?;
    let mut buffer_controls = Vec::new();
    let mut array_controls = BTreeMap::new();
    let mut control_types = BTreeMap::new();
    for item in &mut parsed {
        if let Item::Node(node) | Item::Fx(node) = item {
            let mut params = Vec::new();
            for mut param in std::mem::take(&mut node.params) {
                if matches!(&param.ty,Some(TypeSpec::Unit(t)) if t.name=="buffer") {
                    if let Some(default) = param.default.take() {
                        buffer_controls
                            .push((format!("{}::{}", node.name.name, param.name.name), default));
                        param.default = Some(Expr {
                            kind: ExprKind::Num {
                                value: 0.,
                                unit: None,
                            },
                            span: param.name.span,
                        });
                    }
                    params.push(param);
                } else if matches!(&param.ty, Some(TypeSpec::Array(..))) {
                    if let Some(default) = param.default.clone() {
                        fn expand(
                            value: &Expr,
                            path: &str,
                            node: &str,
                            template: &ParamDef,
                            params: &mut Vec<ParamDef>,
                            paths: &mut Vec<String>,
                        ) -> Expr {
                            if let ExprKind::Array(values) = &value.kind {
                                let mut inner = template.clone();
                                if let Some(TypeSpec::Array(ty, _)) = &template.ty {
                                    inner.ty = Some((**ty).clone());
                                }
                                return Expr {
                                    kind: ExprKind::Array(
                                        values
                                            .iter()
                                            .enumerate()
                                            .map(|(i, v)| {
                                                expand(
                                                    v,
                                                    &format!("{path}[{i}]"),
                                                    node,
                                                    &inner,
                                                    params,
                                                    paths,
                                                )
                                            })
                                            .collect(),
                                    ),
                                    span: value.span,
                                };
                            }
                            let mut param = template.clone();
                            param.name.name = path.to_string();
                            param.default = Some(value.clone());
                            params.push(param);
                            paths.push(path.to_string());
                            Expr {
                                kind: ExprKind::Name(format!("{node}::{path}")),
                                span: value.span,
                            }
                        }
                        let mut paths = Vec::new();
                        let replacement = expand(
                            &default,
                            &param.name.name,
                            &node.name.name,
                            &param,
                            &mut params,
                            &mut paths,
                        );
                        fn replace(e: &mut Expr, name: &str, value: &Expr) {
                            if matches!(&e.kind,ExprKind::Name(n) if n==name) {
                                *e = value.clone();
                            } else {
                                e.for_each_child_mut(&mut |c| replace(c, name, value));
                            }
                        }
                        for statement in &mut node.body {
                            for e in statement.exprs_mut() {
                                replace(e, &param.name.name, &replacement);
                            }
                        }
                        array_controls
                            .insert(format!("{}::{}", node.name.name, param.name.name), paths);
                    }
                } else {
                    params.push(param);
                }
            }
            for param in &params {
                if let Some(ty) = &param.ty
                    && !matches!(ty,TypeSpec::Unit(t) if t.name=="buffer")
                {
                    control_types.insert(
                        format!("{}::{}", node.name.name, param.name.name),
                        ty.clone(),
                    );
                }
            }
            node.params = params;
        }
    }
    let mut items = desugar(parsed);
    let prelude = crate::prelude::needed(source, &items);
    items.extend(prelude);
    let mut errors = Vec::new();

    let tuning = tuning_of(&items).unwrap_or_else(|d| {
        errors.push(d);
        Tuning::default()
    });
    // Musical time is the client's: a definition is compiled against a fixed tempo it cannot name.
    let tempo = Tempo {
        bpm: DEFAULT_BPM,
        beats_per_bar: 4.0,
        beat_unit: 4,
        tuning,
    };

    // Top-level names first, so locals can be kept from shadowing them.
    let mut names = Names::default();
    names.samples = options.samples.clone();
    names.node_outputs = options.node_outputs.clone();
    for item in &items {
        let (ident, kind) = match item {
            Item::Control { name, .. } => (name, "control"),
            Item::Instr(d) => (&d.name, "instrument"),
            Item::MidiOut(d) => (&d.name, "instrument"),
            Item::Fn(d) => (&d.name, "function"),
            Item::Const { name, .. } => (name, "constant"),
            Item::Bus(d) => (&d.name, "bus"),
            Item::Buffer(d) => (&d.name, "buffer"),
            Item::Node(d) => (&d.name, "node"),
            Item::Fx(d) => (&d.name, "fx"),
            Item::Sample(d) => (&d.name, if d.is_kit { "kit" } else { "sample" }),
            _ => continue,
        };
        match names.kind_of(&ident.name) {
            None => names.add(&ident.name, kind),
            // a node may play the instrument of its own name, and share it
            Some("instrument" | "sample" | "kit") if matches!(item, Item::Node(t) if plays_its_namesake(t)) =>
                {}
            // and whichever is written first
            Some("node") if matches!(kind, "instrument" | "sample" | "kit") && items.iter().any(
                |other| matches!(other, Item::Node(t) if t.name.name == ident.name && plays_its_namesake(t)),
            ) => {}
            Some(existing) if existing != kind => {
                errors.push(Diagnostic::new(
                    format!(
                        "`{}` is already the name of {} {existing}",
                        ident.name,
                        if existing.starts_with(['a', 'e', 'i', 'o', 'u']) {
                            "an"
                        } else {
                            "a"
                        }
                    ),
                    ident.span,
                ));
            }
            Some(_) => {}
        }
    }

    match master_layout(&items, options.default_layout) {
        Ok(layout) => names.master = layout,
        Err(d) => errors.push(d),
    }

    let registry = Registry::builtin();
    let mut fns: HashMap<String, FnDef> = HashMap::new();
    for item in &items {
        let Item::Fn(def) = item else { continue };
        if let Some(why) = crate::funcs::reserved_function_name(&def.name.name, &registry) {
            errors.push(Diagnostic::new(why, def.name.span));
        } else if fns.insert(def.name.name.clone(), def.clone()).is_some() {
            errors.push(Diagnostic::new(
                format!("`{}` is defined twice", def.name.name),
                def.name.span,
            ));
        }
    }
    // after user functions are known, so `fn adsr` / `fn perc` win over the shortcuts
    errors.extend(env_shortcut::desugar(&mut items, &fns));
    for item in &items {
        match item {
            Item::Fn(def) => {
                names.fns.insert(def.name.name.clone(), def.clone());
            }
            Item::Instr(def) => {
                names.instrs.insert(def.name.name.clone(), def.clone());
            }
            _ => {}
        }
    }

    for item in &items {
        let Item::Control { name, value, .. } = item else {
            continue;
        };
        if names.controls.iter().any(|c| c.name == name.name) {
            errors.push(Diagnostic::new(
                format!("control `{}` is defined twice", name.name),
                name.span,
            ));
            continue;
        }
        match crate::lower::compile_control(
            name,
            value,
            owned_tempo(&name.name, tempo),
            &registry,
            &names,
        ) {
            Ok(mut control) => {
                if let Some(ty) = control_types.get(&control.name) {
                    match crate::funcs::kind_of(ty, tempo) {
                        Ok(crate::funcs::Kind::Range { unit, lo, hi }) if unit == control.unit => {
                            control.range = Some((lo, hi));
                            control.initial = control.initial.clamp(lo as f32, hi as f32);
                        }
                        Ok(kind) if kind.unit().is_none_or(|u| u == control.unit) => {}
                        Ok(_) => errors.push(Diagnostic::new(
                            "node parameter default has the wrong unit",
                            name.span,
                        )),
                        Err(error) => errors.push(error),
                    }
                }
                names.controls.push(control);
            }
            Err(d) => errors.push(d),
        }
    }

    for item in &items {
        let Item::Const { name, value } = item else {
            continue;
        };
        if names.consts.contains_key(&name.name) {
            errors.push(Diagnostic::new(
                format!("`{}` is defined twice", name.name),
                name.span,
            ));
            continue;
        }
        names.samples = owned_samples(options, &name.name);
        match compile_const(value, owned_tempo(&name.name, tempo), &registry, &names) {
            Ok(v) => {
                names.consts.insert(name.name.clone(), v);
            }
            Err(d) => errors.push(d),
        }
    }

    for item in &items {
        let Item::Fn(def) = item else { continue };
        if let Err(d) = check_fn(def, owned_tempo(&def.name.name, tempo), &registry, &names) {
            errors.push(d);
            names.failed_fns.push(def.name.name.clone());
        }
    }

    let mut seen_buses: Vec<&str> = Vec::new();
    for item in &items {
        let Item::Bus(bus) = item else { continue };
        if seen_buses.contains(&bus.name.name.as_str()) {
            errors.push(Diagnostic::new(
                format!("bus `{}` is defined twice", bus.name.name),
                bus.name.span,
            ));
            continue;
        }
        seen_buses.push(&bus.name.name);
        if let Some(expr) = &bus.layout {
            match layout_from_expr(expr) {
                Ok(layout) => {
                    if let Some(i) = names.bus(&bus.name.name) {
                        names.bus_layouts[i] = layout;
                    }
                }
                Err(d) => errors.push(d),
            }
        }
    }

    // Unlike a declared bus (mono by default), an fx's input preserves the
    // complete incoming layout, including every channel read by its graph.
    for input in fx_inputs.values() {
        let bus = names.bus(input).expect("fx input bus was registered");
        names.bus_layouts[bus] = names.master;
    }

    let mut defined = Defined::default();
    let mut samples_state = SampleState::default();
    for item in &items {
        let Item::Buffer(decl) = item else { continue };
        if names.buffers.contains_key(&decl.name.name) {
            errors.push(Diagnostic::new(
                format!("buffer `{}` is defined twice", decl.name.name),
                decl.name.span,
            ));
            continue;
        }
        names.samples = owned_samples(options, &decl.name.name);
        match compile_buffer(
            decl,
            owned_tempo(&decl.name.name, tempo),
            &registry,
            &names,
            options,
        ) {
            Ok((name, buffer)) => {
                let buffer = options
                    .buffers
                    .get(&name)
                    .filter(|old| {
                        old.len() == buffer.len()
                            && old.channels() == buffer.channels()
                            && old.sample_rate() == buffer.sample_rate()
                    })
                    .cloned()
                    .unwrap_or(buffer);
                let id = names.buffers.len();
                names.add(&name, "buffer");
                names.buffers.insert(name, (id, buffer));
            }
            Err(d) => errors.push(d),
        }
    }
    for (name, default) in buffer_controls {
        let ExprKind::Name(resource) = &default.kind else {
            errors.push(Diagnostic::new(
                "buffer parameter default is a named buffer",
                default.span,
            ));
            continue;
        };
        let Some((_, data)) = names.buffers.get(resource) else {
            errors.push(Diagnostic::new(
                "buffer parameter default does not exist",
                default.span,
            ));
            continue;
        };
        let owner = resource
            .split_once("::")
            .map(|(owner, _)| owner)
            .unwrap_or("");
        let mut choices: Vec<_> = names
            .buffers
            .iter()
            .filter(|(n, _)| n.split_once("::").map(|(o, _)| o).unwrap_or("") == owner)
            .map(|(n, (id, b))| (*id, n.clone(), b.clone()))
            .collect();
        choices.sort_by_key(|v| v.0);
        let index = choices
            .iter()
            .position(|(_, _, b)| Arc::ptr_eq(b, data))
            .unwrap();
        if let Some(control) = names.controls.iter_mut().find(|c| c.name == name) {
            control.initial = index as f32;
        }
        let choices = crate::lower::Choices::named(
            choices.into_iter().map(|(_, n, b)| (n, b)).collect(),
            &name,
        );
        names.buffer_controls.insert(name, choices);
    }
    // Samples before instruments so `kick.data` and table-as-sample are available.
    for item in &items {
        let Item::Sample(decl) = item else { continue };
        if matches!(decl.source, SampleSource::Instruments(_)) {
            continue;
        }
        defined.add(&decl.name, &mut errors, || {
            compile_sample(
                decl,
                &owned_samples(options, &decl.name.name),
                owned_tempo(&decl.name.name, tempo),
                &registry,
                &mut names,
                &mut samples_state,
            )
        });
    }
    for (name, sample) in &samples_state.loaded {
        let channels = &sample.channels;
        names.sample_data.insert(name.clone(), channels.clone());
    }
    for item in &items {
        let Item::Instr(def) = item else { continue };
        names.samples = owned_samples(options, &def.name.name);
        defined.add(&def.name, &mut errors, || {
            compile_instr(def, owned_tempo(&def.name.name, tempo), &registry, &names)
                .and_then(|mut i| voice_options(def, &mut i, &mut samples_state).map(|()| i))
                .and_then(|i| crate::parser::reject_unread_constants(&def.body).map(|()| i))
        });
    }
    for item in &items {
        let Item::MidiOut(decl) = item else { continue };
        defined.add(&decl.name, &mut errors, || {
            crate::lower::compile_midi_out(decl, &names)
        });
    }
    for item in &items {
        let Item::Sample(decl) = item else { continue };
        let SampleSource::Instruments(entries) = &decl.source else {
            continue;
        };
        let result = compile_instrument_kit(
            decl,
            entries,
            &defined.instruments,
            &defined.failed,
            &mut samples_state,
            owned_tempo(&decl.name.name, tempo),
        );
        defined.add(&decl.name, &mut errors, || result);
    }
    let Defined {
        instruments,
        mut failed,
    } = defined;

    // Node 0 plays bare instruments, so it may send to any bus an instrument does.
    let implicit = NodeInfo {
        accepts_notes: true,
        taps: Vec::new(),
        name: String::new(),
        instrument: None,
        defaults: BTreeMap::new(),
        def: NodeDef {
            meter: None,
            transition: (0, 0.0),
            reset_state: false,
            controls: Vec::new(),
            chain: None,
            inputs: Vec::new(),
            route: Route::Master,
            voice_sends: {
                let mut sends: Vec<usize> = instruments
                    .iter()
                    .flat_map(|i| i.graph.send_buses())
                    .collect();
                sends.sort_unstable();
                sends.dedup();
                sends
            },
            voice_buffer_writes: {
                let mut w: Vec<usize> = instruments
                    .iter()
                    .flat_map(|i| i.graph.buffer_writes().iter().copied())
                    .collect();
                w.sort_unstable();
                w.dedup();
                w
            },
            voice_buffer_reads: {
                let mut r: Vec<usize> = instruments
                    .iter()
                    .flat_map(|i| i.graph.buffer_reads().iter().copied())
                    .collect();
                r.sort_unstable();
                r.dedup();
                r
            },
        },
    };
    let mut nodes = vec![implicit];
    let mut node_spans = vec![Span::default()];
    for item in &items {
        let (Item::Node(decl) | Item::Fx(decl)) = item else {
            continue;
        };
        let name = decl.name.name.as_str();
        if nodes.iter().skip(1).any(|t| t.name == name) || failed.contains(&name) {
            errors.push(Diagnostic::new(
                format!("node `{name}` is defined twice"),
                decl.name.span,
            ));
            continue;
        }
        names.samples = owned_samples(options, &decl.name.name);
        match compile_node(
            item,
            &instruments,
            &failed,
            &samples_state.names,
            owned_tempo(&decl.name.name, tempo),
            &registry,
            &names,
        ) {
            Ok(Some(mut t)) => {
                if let Some(input) = fx_inputs.get(&decl.name.name) {
                    let bus = names.bus(input).expect("fx input bus was declared");
                    for source in &mut t.def.inputs {
                        if let niminal_engine::ChainInput::It { channel } = *source {
                            *source = niminal_engine::ChainInput::Bus { bus, channel };
                        }
                    }
                }
                nodes.push(t);
                node_spans.push(decl.name.span);
            }
            Ok(None) => failed.push(name),
            Err(d) => {
                errors.push(d);
                failed.push(name);
            }
        }
    }

    let defs: Vec<NodeDef> = nodes.iter().map(|t| t.def.clone()).collect();
    if let Err(cycle) = execution_order(&defs) {
        let path: Vec<&str> = cycle
            .iter()
            .chain(cycle.first())
            .map(|&i| nodes[i].name.as_str())
            .collect();
        errors.push(
            Diagnostic::new(format!("nodes feed back through buses or buffers: {}", path.join(" -> ")), node_spans[cycle[0]])
                .with_help("a bus or shared buffer has to be written before it is read; break a bus loop with `late`, by sending a late copy: `echo += late(out)`"),
        );
    }

    errors.retain(|d| !d.is_quiet());
    if errors.is_empty() {
        let mut program = Program {
            namespaces: options.namespaces.clone(),
            namespace_samples: options.namespace_samples.clone(),
            resource_controls: names
                .buffer_controls
                .iter()
                .map(|(name, choices)| (name.clone(), choices.key))
                .collect(),
            array_controls,
            bus_meters: Vec::new(),
            profile: Arc::new(niminal_engine::Profile::new(nodes.len())),
            source: source.to_string(),
            samples: options.samples.clone(),
            taps: nodes
                .iter()
                .flat_map(|t| t.taps.clone())
                .chain(instruments.iter().flat_map(|i| i.taps.clone()))
                .collect(),
            controls: names.controls,
            tempo,
            master: names.master,
            instruments,
            buses: names.buses,
            bus_layouts: names.bus_layouts,
            buffers: {
                let mut bufs: Vec<_> = names
                    .buffers
                    .into_iter()
                    .map(|(name, (id, buffer))| (id, name, buffer))
                    .collect();
                bufs.sort_by_key(|(id, _, _)| *id);
                bufs.into_iter()
                    .map(|(_, name, buffer)| (name, buffer))
                    .collect()
            },
            nodes,
            sample_names: samples_state.names,
            choke_groups: samples_state.groups,
        };
        if !options.defer_latency {
            program.compensate_latency();
        }
        Ok(program)
    } else {
        Err(errors)
    }
}
