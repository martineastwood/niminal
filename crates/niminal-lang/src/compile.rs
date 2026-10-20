//! Program-level compilation: tempo, functions, instruments and notes.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use niminal_engine::{Route, TrackDef, execution_order};
use niminal_score::{MAX_DEGREES, Tempo, Time, Tuning, Value};

use crate::ast::*;
use crate::diag::{Diagnostic, Span, closest};
use crate::env_shortcut;
use crate::layout::Layout;
use crate::lower::{
    Datum, Member, Mixer, Names, check_fn, compile_chain, compile_const, compile_instr,
    compile_sampler, layout_from_expr, unknown_unit,
};
use crate::opcodes::Registry;
use crate::parser;
use crate::program::{Instrument, Program, TrackInfo};
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
    /// Defer topology-dependent compensation until the host has connected processor instances.
    pub defer_latency: bool,
    /// The output layout when the source has no `config { channels: ... }`.
    pub default_layout: Layout,
    /// Where the files named by `sample` and `kit` are found.
    pub samples: Samples,
    /// Existing shared resources to retain when a definition keeps its name, length and rate.
    pub buffers: HashMap<String, Arc<niminal_engine::Buffer>>,
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
            samples: Samples::default(),
            buffers: HashMap::new(),
        }
    }
}

/// Spell out the short forms: a bus with a body is also a track that reads the bus and processes it.
fn desugar(items: Vec<Item>) -> Vec<Item> {
    let mut out = Vec::with_capacity(items.len());
    for mut item in items {
        match &mut item {
            Item::Bus(bus) => {
                if let Some(mut body) = bus.body.take() {
                    for stmt in &mut body {
                        for e in stmt.exprs_mut() {
                            rename(e, "it", &bus.name.name);
                        }
                    }
                    // a name no one can type, so it never meets a track of the user's
                    let name = Ident {
                        name: format!("{}.fx", bus.name.name),
                        span: bus.name.span,
                    };
                    out.push(Item::Track(TrackDecl {
                        name,
                        params: Vec::new(),
                        body,
                        options: Vec::new(),
                        span: bus.name.span,
                    }));
                }
            }
            Item::Track(track) => {
                // a parameter is a control called `track::param`, and the body reads it by its short name
                for param in std::mem::take(&mut track.params) {
                    let Some(default) = param.default else {
                        continue;
                    };
                    let long = format!("{}::{}", track.name.name, param.name.name);
                    for stmt in &mut track.body {
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
                    });
                }
            }
            _ => {}
        }
        out.push(item);
    }
    out
}

/// A track that does nothing but play the instrument of its own name.
fn plays_its_namesake(track: &TrackDecl) -> bool {
    matches!(track.body.as_slice(), [Stmt::Bind { name, value }]
        if name.name == "instrument" && matches!(&value.kind, ExprKind::Name(n) if *n == track.name.name))
}

/// Every use of the name `from` in `e` becomes `to`.
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
    crate::validate_dsp(source).map_err(|d| vec![d])?;
    let mut parsed = parser::parse(source).map_err(|d| vec![d])?;
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
                            && layout_from_expr(value).map_err(|d| vec![d])?
                                != options.default_layout
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
    let mut buffer_controls = Vec::new();
    let mut array_controls = BTreeMap::new();
    let mut control_types = BTreeMap::new();
    for item in &mut parsed {
        if let Item::Track(track) = item {
            let mut params = Vec::new();
            for mut param in std::mem::take(&mut track.params) {
                if matches!(&param.ty,Some(TypeSpec::Unit(t)) if t.name=="buffer") {
                    if let Some(default) = param.default.take() {
                        buffer_controls
                            .push((format!("{}::{}", track.name.name, param.name.name), default));
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
                            track: &str,
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
                                                    track,
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
                                kind: ExprKind::Name(format!("{track}::{path}")),
                                span: value.span,
                            }
                        }
                        let mut paths = Vec::new();
                        let replacement = expand(
                            &default,
                            &param.name.name,
                            &track.name.name,
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
                        for statement in &mut track.body {
                            for e in statement.exprs_mut() {
                                replace(e, &param.name.name, &replacement);
                            }
                        }
                        array_controls
                            .insert(format!("{}::{}", track.name.name, param.name.name), paths);
                    }
                } else {
                    params.push(param);
                }
            }
            for param in &params {
                if let Some(ty) = &param.ty
                    && !matches!(ty,TypeSpec::Unit(t) if t.name=="buffer") {
                        control_types.insert(
                            format!("{}::{}", track.name.name, param.name.name),
                            ty.clone(),
                        );
                    }
            }
            track.params = params;
        }
    }
    let mut items = desugar(parsed);
    let mut errors = Vec::new();

    let tuning = tuning_of(&items).unwrap_or_else(|d| {
        errors.push(d);
        Tuning::default()
    });
    // Musical time is the client's: a definition is compiled against a fixed tempo it cannot name.
    let tempo = Tempo {
        bpm: DEFAULT_BPM,
        beats_per_bar: 4.0,
        tuning,
    };

    // Top-level names first, so locals can be kept from shadowing them.
    let mut names = Names::default();
    names.samples = options.samples.clone();
    for item in &items {
        let (ident, kind) = match item {
            Item::Control { name, .. } => (name, "control"),
            Item::Instr(d) => (&d.name, "instrument"),
            Item::MidiOut(d) => (&d.name, "instrument"),
            Item::Fn(d) => (&d.name, "function"),
            Item::Const { name, .. } => (name, "constant"),
            Item::Bus(d) => (&d.name, "bus"),
            Item::Buffer(d) => (&d.name, "buffer"),
            Item::Track(d) => (&d.name, "track"),
            Item::Sample(d) => (&d.name, if d.is_kit { "kit" } else { "sample" }),
            _ => continue,
        };
        match names.kind_of(&ident.name) {
            None => names.add(&ident.name, kind),
            // a track may play the instrument of its own name, and share it
            Some("instrument") if matches!(item, Item::Track(t) if plays_its_namesake(t)) => {}
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
        if registry.find(&def.name.name).is_some()
            || matches!(def.name.name.as_str(), "reset_on" | "after")
        {
            errors.push(Diagnostic::new(
                format!(
                    "`{}` is a built-in function, so it can't be defined again",
                    def.name.name
                ),
                def.name.span,
            ));
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
        let Item::Control { name, value } = item else {
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
                            "track parameter default has the wrong unit",
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
        defined.add(&decl.name, &mut errors, || {
            compile_sample(
                decl,
                &owned_samples(options, &decl.name.name),
                owned_tempo(&decl.name.name, tempo),
                &registry,
                &names,
                &mut samples_state,
            )
        });
    }
    for (name, (channels, _)) in &samples_state.loaded {
        names.sample_data.insert(name.clone(), channels.clone());
    }
    for item in &items {
        let Item::Instr(def) = item else { continue };
        names.samples = owned_samples(options, &def.name.name);
        defined.add(&def.name, &mut errors, || {
            compile_instr(def, owned_tempo(&def.name.name, tempo), &registry, &names)
                .and_then(|mut i| voice_options(def, &mut i, &mut samples_state).map(|()| i))
        });
    }
    for item in &items {
        let Item::MidiOut(decl) = item else { continue };
        defined.add(&decl.name, &mut errors, || {
            crate::lower::compile_midi_out(decl, &names)
        });
    }
    let Defined {
        instruments,
        mut failed,
    } = defined;

    // Track 0 plays bare instruments, so it may send to any bus an instrument does.
    let implicit = TrackInfo {
        accepts_notes: true,
        taps: Vec::new(),
        name: String::new(),
        instrument: None,
        defaults: BTreeMap::new(),
        def: TrackDef {
            meter: None,
            transition: (0, 0.0),
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
    let mut tracks = vec![implicit];
    let mut track_spans = vec![Span::default()];
    for item in &items {
        let Item::Track(decl) = item else { continue };
        let name = decl.name.name.as_str();
        if tracks.iter().skip(1).any(|t| t.name == name) || failed.contains(&name) {
            errors.push(Diagnostic::new(
                format!("track `{name}` is defined twice"),
                decl.name.span,
            ));
            continue;
        }
        names.samples = owned_samples(options, &decl.name.name);
        match compile_track(
            decl,
            &instruments,
            &failed,
            &samples_state.names,
            owned_tempo(&decl.name.name, tempo),
            &registry,
            &names,
        ) {
            Ok(Some(t)) => {
                tracks.push(t);
                track_spans.push(decl.name.span);
            }
            Ok(None) => failed.push(name),
            Err(d) => {
                errors.push(d);
                failed.push(name);
            }
        }
    }

    let defs: Vec<TrackDef> = tracks.iter().map(|t| t.def.clone()).collect();
    if let Err(cycle) = execution_order(&defs) {
        let path: Vec<&str> = cycle
            .iter()
            .chain(cycle.first())
            .map(|&i| tracks[i].name.as_str())
            .collect();
        errors.push(
            Diagnostic::new(format!("tracks feed back through buses or buffers: {}", path.join(" -> ")), track_spans[cycle[0]])
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
            source: source.to_string(),
            samples: options.samples.clone(),
            taps: tracks
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
            tracks,
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

/// A track's chain, plus the instrument named by its `instrument = ...` line.
/// `None` when that instrument failed to compile, which is already reported.
fn compile_track(
    decl: &TrackDecl,
    instruments: &[Instrument],
    failed: &[&str],
    sample_names: &[String],
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
) -> Res<Option<TrackInfo>> {
    let mut instrument_line: Option<&Expr> = None;
    let mut mixer = Mixer::default();
    let mut rest: Vec<&Stmt> = Vec::new();
    for stmt in &decl.body {
        match stmt {
            Stmt::Bind { name, value } if matches!(name.name.as_str(), "gain" | "pan") => {
                let line = if name.name == "gain" {
                    &mut mixer.gain
                } else {
                    &mut mixer.pan
                };
                if line.is_some() {
                    return Err(Diagnostic::new(
                        format!("a track has only one `{}` line", name.name),
                        name.span,
                    ));
                }
                *line = Some(value);
            }
            Stmt::Bind { name, value } if name.name == "instrument" => {
                if instrument_line.is_some() {
                    return Err(Diagnostic::new(
                        "a track has only one instrument",
                        name.span,
                    ));
                }
                instrument_line = Some(value);
            }
            other => rest.push(other),
        }
    }

    let mut instrument = None;
    let mut defaults = BTreeMap::new();
    if let Some(value) = instrument_line {
        match resolve_instrument(value, instruments, failed, sample_names, tempo)? {
            None => return Ok(None),
            Some((index, args)) => {
                defaults = args;
                instrument = Some(index);
            }
        }
    }

    let chain = compile_chain(
        &decl.name,
        &rest,
        instrument.is_some(),
        &mixer,
        tempo,
        registry,
        names,
    )?;
    let voice_sends = instrument
        .map(|i| instruments[i].graph.send_buses())
        .unwrap_or_default();
    let voice_buffer_writes = instrument
        .map(|i| instruments[i].graph.buffer_writes().to_vec())
        .unwrap_or_default();
    let voice_buffer_reads = instrument
        .map(|i| instruments[i].graph.buffer_reads().to_vec())
        .unwrap_or_default();
    Ok(Some(TrackInfo {
        accepts_notes: instrument.is_some(),
        taps: chain.taps.clone(),
        name: decl.name.name.clone(),
        instrument,
        defaults,
        def: TrackDef {
            meter: None,
            transition: (0, 0.0),
            controls: Vec::new(),
            chain: chain.graph,
            inputs: chain.inputs,
            route: chain.route,
            voice_sends,
            voice_buffer_writes,
            voice_buffer_reads,
        },
    }))
}

/// `pluck`, `pluck(bright: 0.6)`, `drums.snare`, or `drums.snare(pitch: -2st)`.
/// `None` when the instrument failed earlier.
fn resolve_instrument(
    value: &Expr,
    instruments: &[Instrument],
    failed: &[&str],
    sample_names: &[String],
    tempo: Tempo,
) -> Res<Option<(usize, BTreeMap<String, Value>)>> {
    let help = "for example `instrument = pluck`, `instrument = pluck(bright: 0.6)`, or `instrument = drums.snare`";
    let bad = || Diagnostic::new("expected an instrument here", value.span).with_help(help);

    let (name, member, span, args): (&str, Option<&str>, Span, &[Arg]) = match &value.kind {
        ExprKind::Name(n) => (n.as_str(), None, value.span, &[][..]),
        ExprKind::Call { name, args } => {
            let receivers: Vec<_> = args.iter().filter(|a| a.receiver).collect();
            if receivers.is_empty() {
                (name.name.as_str(), None, name.span, args.as_slice())
            } else if receivers.len() == 1
                && let ExprKind::Name(kit) = &receivers[0].value.kind
            {
                // `drums.snare(...)` is parsed as `snare(drums, ...)`.
                let start = args.iter().position(|a| !a.receiver).unwrap_or(args.len());
                (
                    kit.as_str(),
                    Some(name.name.as_str()),
                    name.span,
                    &args[start..],
                )
            } else {
                return Err(bad());
            }
        }
        _ => return Err(bad()),
    };

    if failed.contains(&name) {
        return Ok(None);
    }
    let Some(index) = instruments.iter().position(|i| i.name == name) else {
        let mut d = Diagnostic::new(format!("no instrument named `{name}`"), span);
        if let Some(c) = closest(name, instruments.iter().map(|i| i.name.as_str())) {
            d = d.with_help(format!("did you mean `{c}`?"));
        }
        return Err(d);
    };
    let instr = &instruments[index];
    let mut defaults = literal_args(args, instr, sample_names, tempo)?;
    if let Some(member) = member {
        let local = instr
            .members
            .iter()
            .position(|m| m == member)
            .ok_or_else(|| {
                let mut d = Diagnostic::new(
                    format!("there is no sample `{member}` in the kit `{name}`"),
                    span,
                );
                if instr.members.is_empty() {
                    d = d.with_help(format!("`{name}` isn't a kit"));
                } else if let Some(c) = closest(member, instr.members.iter().map(String::as_str)) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                d
            })?;
        if defaults.contains_key("sample") {
            return Err(Diagnostic::new(
                "the sample is already chosen by the name after the kit",
                span,
            )
            .with_help(format!(
                "write `{name}.{member}` or `{name}(sample: ...)`, not both"
            )));
        }
        // The kit's `sample` parameter counts members from zero.
        defaults.insert("sample".into(), Value::Num(local as f64));
    }
    Ok(Some((index, defaults)))
}

/// `mono bass(...) with(legato: true, glide: 80ms, priority: lowest)` and
/// `instr pad(...) with(max: 8, steal: quietest, same_note: release, choke: group)`.
fn voice_options(def: &InstrDef, instrument: &mut Instrument, state: &mut SampleState) -> Res<()> {
    let mut max = def.mono.then_some(1);
    let mono_only = |option: &Ident| {
        Diagnostic::new(
            format!("`{}` is for `mono` instruments", option.name),
            option.span,
        )
        .with_help("write `mono` where `instr` is")
    };
    let mut mono_option = None;
    for arg in &def.options {
        let Some(option) = &arg.name else {
            return Err(Diagnostic::new(
                "an option is a name and a value, as in `max: 8`",
                arg.value.span,
            ));
        };
        let value = &arg.value;
        match option.name.as_str() {
            "max" => {
                if def.mono {
                    return Err(Diagnostic::new(
                        "a `mono` instrument is one voice, so it has no `max`",
                        option.span,
                    ));
                }
                match &value.kind {
                    ExprKind::Num {
                        value: n,
                        unit: None,
                    } if n.fract() == 0.0 && *n >= 1.0 && *n <= 4096.0 => max = Some(*n as usize),
                    _ => {
                        return Err(Diagnostic::new(
                            "`max` is a whole number of voices, from 1",
                            value.span,
                        ));
                    }
                }
            }
            "steal" => match &value.kind {
                ExprKind::Name(n) if n == "oldest" => {}
                ExprKind::Name(n) if n == "quietest" => instrument.steal_quietest = true,
                _ => {
                    return Err(Diagnostic::new(
                        "`steal` is `oldest` or `quietest`",
                        value.span,
                    ));
                }
            },
            "same_note" => match &value.kind {
                ExprKind::Name(n) if n == "release" => instrument.release_same_note = true,
                _ => return Err(Diagnostic::new("`same_note` is `release`", value.span)),
            },
            "choke" => match &value.kind {
                ExprKind::Name(group) => {
                    let name = scoped_group(&instrument.name, group);
                    if !state.groups.contains(&name) {
                        state.groups.push(name.clone());
                    }
                    instrument.chokes = vec![Some(niminal_engine::instance_id(&name))];
                }
                _ => {
                    return Err(
                        Diagnostic::new("expected the name of a choke group", value.span)
                            .with_help("for example `with(choke: bass)`"),
                    );
                }
            },
            "legato" => {
                if !def.mono {
                    return Err(mono_only(option));
                }
                instrument.legato = match &value.kind {
                    ExprKind::Name(n) if n == "true" => true,
                    ExprKind::Name(n) if n == "false" => false,
                    _ => return Err(Diagnostic::new("`legato` is `true` or `false`", value.span)),
                };
                mono_option = Some(option);
            }
            "priority" => {
                if !def.mono {
                    return Err(mono_only(option));
                }
                instrument.priority = match &value.kind {
                    ExprKind::Name(n) if n == "last" => niminal_engine::Priority::Last,
                    ExprKind::Name(n) if n == "lowest" => niminal_engine::Priority::Lowest,
                    ExprKind::Name(n) if n == "highest" => niminal_engine::Priority::Highest,
                    _ => {
                        return Err(Diagnostic::new(
                            "`priority` is `last`, `lowest` or `highest`",
                            value.span,
                        ));
                    }
                };
                mono_option = Some(option);
            }
            // read while the instrument is compiled
            "tail" => {}
            "pitch" => {
                let ExprKind::Name(name) = &value.kind else {
                    return Err(Diagnostic::new("`pitch` is the name of a parameter", value.span)
                        .with_help("for example `with(pitch: tone)`, for `instr bass(tone: hz = 55hz)`"));
                };
                let Some(param) = instrument.params.iter().find(|p| p.name == *name) else {
                    return Err(Diagnostic::new(
                        format!("the instrument has no parameter `{name}` to take the pitch"),
                        value.span,
                    ));
                };
                if param.unit != crate::Unit::Hz {
                    return Err(Diagnostic::new(
                        format!("`{name}` is the pitch, so it is in `hz`"),
                        value.span,
                    ));
                }
                instrument.pitch = name.clone();
            }
            "glide" => {
                if !def.mono {
                    return Err(mono_only(option));
                }
                instrument.glide = Some(literal_time(value)?);
                mono_option = Some(option);
            }
            other => {
                let mut d = Diagnostic::new(
                    format!("an instrument has no option `{other}`"),
                    option.span,
                );
                d = match closest(other, ["max", "steal", "same_note", "choke", "legato", "priority", "glide", "quantize", "tail", "pitch"]) {
                    Some(c) => d.with_help(format!("did you mean `{c}`?")),
                    None => d.with_help("the options are `max`, `steal`, `same_note`, `choke`, `tail`, `quantize`, `pitch`, and for `mono` instruments `legato`, `priority` and `glide`"),
                };
                return Err(d);
            }
        }
    }
    if let Some(option) = mono_option
        && !instrument.params.iter().any(|p| p.name == instrument.pitch)
    {
        return Err(Diagnostic::new(
            format!(
                "`{}` moves the pitch, and this instrument has no `{}` parameter",
                option.name, instrument.pitch
            ),
            option.span,
        )
        .with_help(format!("give it a `{}: hz` parameter", instrument.pitch)));
    }
    if instrument.priority != niminal_engine::Priority::Last && !instrument.legato {
        return Err(Diagnostic::new(
            "`priority` decides which held note a legato voice sounds",
            def.options
                .iter()
                .find_map(|a| a.name.as_ref().filter(|n| n.name == "priority"))
                .map_or(def.options[0].value.span, |n| n.span),
        )
        .with_help("add `legato: true`"));
    }
    instrument.max_voices = max;
    Ok(())
}

/// What the `sample` and `kit` declarations of a project define between them.
#[derive(Default)]
struct SampleState {
    /// The names of the samples in kits, which a pattern may use.
    names: Vec<String>,
    /// The names of choke groups.
    groups: Vec<String>,
    /// Lone samples already loaded, for `.slices` to cut up and `.data` to expose.
    loaded: HashMap<String, (Vec<Arc<niminal_engine::Table>>, f64)>,
}

/// The kit made by cutting `data` into `count` equal pieces called `s0`, `s1`...
fn slice_members(
    decl: &SampleDecl,
    sample: &Ident,
    count: f64,
    state: &SampleState,
) -> Res<Vec<Member>> {
    let Some((data, speed)) = state.loaded.get(&sample.name) else {
        let mut d = Diagnostic::new(
            format!("no sample named `{}` to slice", sample.name),
            sample.span,
        );
        if let Some(c) = closest(&sample.name, state.loaded.keys().map(String::as_str)) {
            d = d.with_help(format!("did you mean `{c}`?"));
        } else {
            d = d.with_help("declare it first with `sample name = \"file.wav\"`");
        }
        return Err(d);
    };
    let frames = data.first().map_or(0, |c| c.len());
    if count.fract() != 0.0 || count < 1.0 || count as usize > frames {
        return Err(Diagnostic::new(
            format!("a sample of {frames} frames can be cut into 1 to {frames} slices"),
            decl.source_span,
        ));
    }
    let count = count as usize;
    let each = frames / count;
    Ok((0..count)
        .map(|i| Member {
            name: format!("s{i}"),
            channels: data.clone(),
            slice: Some((
                i * each,
                if i + 1 == count {
                    frames
                } else {
                    (i + 1) * each
                },
            )),
            speed: *speed,
            loop_mode: niminal_engine::ops::LoopMode::None,
        })
        .collect())
}

/// A `sample` or `kit` declaration as an instrument.
fn compile_sample(
    decl: &SampleDecl,
    samples: &Samples,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
    state: &mut SampleState,
) -> Res<Instrument> {
    let mut root = DEFAULT_ROOT_HZ;
    let mut chokes: Vec<&Arg> = Vec::new();
    let (mut duration, mut fit): (Option<f64>, Option<&Arg>) = (None, None);
    for arg in &decl.options {
        let Some(key) = &arg.name else {
            return Err(Diagnostic::new(
                "options are written `name: value`",
                arg.value.span,
            ));
        };
        match key.name.as_str() {
            "root" => {
                root = literal_value(&arg.value)?
                    .as_hz_in(&tempo.tuning)
                    .map_err(|e| {
                        Diagnostic::new(
                            format!("`root` should be a note or a frequency: {e}"),
                            arg.value.span,
                        )
                    })? as f32;
            }
            "choke" => chokes.push(arg),
            "beats" => match &arg.value.kind {
                ExprKind::Num { value, unit: None } if *value > 0.0 => {
                    duration = Some(*value * 60.0 / tempo.bpm)
                }
                _ => {
                    return Err(Diagnostic::new(
                        "`beats` is how many beats the loop lasts, such as `4`",
                        arg.value.span,
                    ));
                }
            },
            "duration" => {
                let seconds = literal_time(&arg.value)?.to_seconds(tempo);
                if !seconds.is_finite() || seconds <= 0.0 {
                    return Err(Diagnostic::new(
                        "sample duration must be positive",
                        arg.value.span,
                    ));
                }
                duration = Some(seconds);
            }
            "fit" => fit = Some(arg),
            "loop" => {} // handled by loop_mode_of
            other => {
                let mut d =
                    Diagnostic::new(format!("`{other}` isn't an option of a sample"), key.span);
                d = match closest(other, ["root", "choke", "beats", "duration", "fit", "loop"]) {
                    Some(c) => d.with_help(format!("did you mean `{c}`?")),
                    None => d.with_help(
                        "the options are `root`, `choke`, `beats`, `duration`, `fit` and `loop`",
                    ),
                };
                return Err(d);
            }
        }
    }
    let fit_error = |d: Diagnostic| {
        d.with_help(
            "for example `with(beats: 4, fit: rate)` plays a 4-beat loop in time with the tempo",
        )
    };
    if let Some(arg) = fit {
        match (&arg.value.kind, duration) {
            (ExprKind::Name(n), Some(_)) if n == "rate" => {}
            (ExprKind::Name(n), _) if n == "slice" || n == "stretch" => {
                return Err(Diagnostic::new(
                    format!("`fit: {n}` isn't supported yet"),
                    arg.value.span,
                )
                .with_help("`fit: rate` speeds the loop up or down with the tempo"));
            }
            (ExprKind::Name(n), None) if n == "rate" => {
                return Err(fit_error(Diagnostic::new(
                    "`fit: rate` needs to know the loop's length",
                    arg.value.span,
                )));
            }
            _ => {
                return Err(fit_error(Diagnostic::new(
                    "`fit` should be `rate`",
                    arg.value.span,
                )));
            }
        }
        if !matches!(decl.source, SampleSource::Path(_)) || decl.is_kit {
            return Err(Diagnostic::new(
                "only a single sample can be fitted to the tempo",
                arg.value.span,
            ));
        }
    }
    let file_error = |e: String| Diagnostic::new(e, decl.source_span);
    let members: Vec<Member> = match (&decl.source, decl.is_kit) {
        (SampleSource::Path(path), true) => samples
            .load_kit(path)
            .map_err(file_error)?
            .into_iter()
            .map(|(name, channels)| Member {
                name,
                channels,
                slice: None,
                speed: 1.0,
                loop_mode: niminal_engine::ops::LoopMode::None,
            })
            .collect(),
        (SampleSource::Path(path), false) => {
            let channels = samples.load(path).map_err(file_error)?;
            let frames = channels.first().map_or(0, |c| c.len());
            let rate = channels
                .first()
                .map_or(48_000.0, |c| c.sample_rate_or(48_000.0));
            // Fitting to the tempo: the loop's own length in seconds against the time its beats take.
            let speed = match (fit, duration) {
                (Some(_), Some(b)) => (frames as f64 / f64::from(rate)) / b,
                _ => 1.0,
            };
            state
                .loaded
                .insert(decl.name.name.clone(), (channels.clone(), speed));
            vec![Member {
                name: decl.name.name.clone(),
                channels,
                slice: None,
                speed,
                loop_mode: loop_mode_of(&decl.options)?,
            }]
        }
        (SampleSource::Slices { sample, count }, _) => {
            let mut members = slice_members(decl, sample, *count, state)?;
            let mode = loop_mode_of(&decl.options)?;
            for m in &mut members {
                m.loop_mode = mode;
            }
            members
        }
        (SampleSource::Table(expr), false) => {
            let value = compile_const(expr, tempo, registry, names)?;
            let table = table_from_const(value, decl.source_span)?;
            let channels = vec![table];
            state
                .loaded
                .insert(decl.name.name.clone(), (channels.clone(), 1.0));
            vec![Member {
                name: decl.name.name.clone(),
                channels,
                slice: None,
                speed: 1.0,
                loop_mode: loop_mode_of(&decl.options)?,
            }]
        }
        (SampleSource::Table(_), true) => {
            return Err(Diagnostic::new(
                "a kit is a folder of files or `.slices`, not a table",
                decl.source_span,
            ));
        }
    };
    if decl.is_kit {
        for member in &members {
            let name = &member.name;
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                || name.starts_with(|c: char| c.is_ascii_digit())
            {
                return Err(Diagnostic::new(format!("the file `{name}.wav` can't be named in a pattern"), decl.source_span)
                    .with_help("name sample files with letters, digits and underscores, starting with a letter"));
            }
            if !state.names.contains(name) {
                state.names.push(name.clone());
            }
        }
    }
    let mut instrument = compile_sampler(
        &decl.name,
        &members,
        decl.is_kit,
        root,
        tempo,
        registry,
        names,
    )?;

    // `choke: hats` for a sample; `choke: hats(open, closed)` for samples in a kit.
    instrument.chokes = vec![None; members.len()];
    let mut group_of = |name: &str| -> u64 {
        let name = scoped_group(&decl.name.name, name);
        if !state.groups.contains(&name) {
            state.groups.push(name.clone());
        }
        niminal_engine::instance_id(&name)
    };
    for arg in chokes {
        let value = &arg.value;
        match (&value.kind, decl.is_kit) {
            (ExprKind::Name(group), false) => instrument.chokes[0] = Some(group_of(group)),
            (ExprKind::Call { name: group, args }, true) => {
                let id = group_of(&group.name);
                for member in args {
                    let ExprKind::Name(m) = &member.value.kind else {
                        return Err(Diagnostic::new(
                            "expected the name of a sample in the kit",
                            member.value.span,
                        ));
                    };
                    let Some(i) = members.iter().position(|x| x.name == *m) else {
                        let mut d = Diagnostic::new(
                            format!("there is no sample `{m}` in the kit `{}`", decl.name.name),
                            member.value.span,
                        );
                        if let Some(c) = closest(m, members.iter().map(|x| x.name.as_str())) {
                            d = d.with_help(format!("did you mean `{c}`?"));
                        }
                        return Err(d);
                    };
                    instrument.chokes[i] = Some(id);
                }
            }
            (_, false) => {
                return Err(
                    Diagnostic::new("expected the name of a choke group", value.span)
                        .with_help("for example `with(choke: hats)`"),
                );
            }
            (_, true) => {
                return Err(
                    Diagnostic::new("a kit names the samples in each group", value.span)
                        .with_help("for example `with(choke: hats(hat_open, hat_closed))`"),
                );
            }
        }
    }
    Ok(instrument)
}

/// The pitch (c4) at which a sample plays as recorded unless it says otherwise.
const DEFAULT_ROOT_HZ: f32 = 261.625_56;

fn loop_mode_of(options: &[Arg]) -> Res<niminal_engine::ops::LoopMode> {
    for arg in options {
        if arg.name.as_ref().is_some_and(|n| n.name == "loop") {
            return match &arg.value.kind {
                ExprKind::Name(n) if n == "none" || n == "false" => {
                    Ok(niminal_engine::ops::LoopMode::None)
                }
                ExprKind::Name(n) if n == "forward" || n == "true" => {
                    Ok(niminal_engine::ops::LoopMode::Forward)
                }
                ExprKind::Name(n) if n == "bidirectional" || n == "pingpong" => {
                    Ok(niminal_engine::ops::LoopMode::Bidirectional)
                }
                _ => Err(Diagnostic::new(
                    "`loop` is `none`, `forward` or `bidirectional`",
                    arg.value.span,
                )),
            };
        }
    }
    Ok(niminal_engine::ops::LoopMode::None)
}

fn table_from_const(value: Datum, span: Span) -> Res<Arc<niminal_engine::Table>> {
    match value {
        Datum::Table(t) => Ok(t),
        Datum::Array(items) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items.iter() {
                match item {
                    Datum::Sig(s) if s.konst().is_some() && s.unit() == crate::unit::Unit::Num => {
                        values.push(s.konst().unwrap());
                    }
                    _ => {
                        return Err(Diagnostic::new(
                            "a sample table is a one-dimensional array of plain numbers",
                            span,
                        ));
                    }
                }
            }
            if values.is_empty() {
                return Err(Diagnostic::new("a table can't be empty", span));
            }
            Ok(Arc::new(niminal_engine::Table::from_f64(values, None)))
        }
        _ => Err(Diagnostic::new(
            "expected a table of plain numbers known while compiling",
            span,
        )),
    }
}

fn compile_buffer(
    decl: &BufferDecl,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
    options: &CompileOptions,
) -> Res<(String, Arc<niminal_engine::Buffer>)> {
    if let ExprKind::Call { name, args } = &decl.value.kind
        && name.name == "zeros" {
            let mut frames = None;
            let mut channels = 1usize;
            for arg in args {
                let Datum::Sig(value) = compile_const(&arg.value, tempo, registry, names)? else {
                    return Err(Diagnostic::new(
                        "buffer size is a constant number",
                        arg.value.span,
                    ));
                };
                let Some(n) = value.konst() else {
                    return Err(Diagnostic::new("buffer size is constant", arg.value.span));
                };
                if value.unit() != crate::unit::Unit::Num
                    || !n.is_finite()
                    || n < 1.
                    || n.fract() != 0.
                {
                    return Err(Diagnostic::new(
                        "buffer dimensions are positive integers",
                        arg.value.span,
                    ));
                }
                match arg.name.as_ref().map(|a| a.name.as_str()) {
                    None if frames.is_none() => frames = Some(n as usize),
                    Some("channels") if n <= 16. => channels = n as usize,
                    _ => {
                        return Err(Diagnostic::new(
                            "zeros takes frames and channels: 1–16",
                            arg.value.span,
                        ));
                    }
                }
            }
            let frames =
                frames.ok_or_else(|| Diagnostic::new("zeros needs a frame count", decl.span))?;
            return allocate_buffer(decl, frames, channels, None, names, options);
        }
    let value = compile_const(&decl.value, tempo, registry, names)?;
    let buffer = match value {
        Datum::Table(t) => niminal_engine::Buffer::from_table(&t),
        Datum::Array(_) => {
            let table = table_from_const(value, decl.value.span)?;
            niminal_engine::Buffer::from_table(&table)
        }
        Datum::Sig(s) => {
            // Durations use the running engine sample rate.
            if s.unit() == crate::unit::Unit::Time {
                let Some(secs) = s.konst() else {
                    return Err(Diagnostic::new(
                        "a buffer's length must be known while compiling",
                        decl.value.span,
                    ));
                };
                let len = (secs * f64::from(options.sample_rate)).round().max(1.0) as usize;
                return allocate_buffer(decl, len, 1, Some(options.sample_rate), names, options);
            } else if s.unit() == crate::unit::Unit::Num {
                let Some(n) = s.konst() else {
                    return Err(Diagnostic::new(
                        "a buffer's length must be known while compiling",
                        decl.value.span,
                    ));
                };
                if n.fract() != 0.0 || n < 1.0 {
                    return Err(Diagnostic::new(
                        "a buffer length is a whole number of samples",
                        decl.value.span,
                    ));
                }
                return allocate_buffer(decl, n as usize, 1, None, names, options);
            } else {
                return Err(Diagnostic::new(
                    "a buffer is sized with `zeros(n)`, a table, or a duration such as `4bars`",
                    decl.value.span,
                ));
            }
        }
        _ => {
            return Err(Diagnostic::new(
                "a buffer is sized with `zeros(n)`, a table, or a duration such as `4bars`",
                decl.value.span,
            ));
        }
    };
    Ok((decl.name.name.clone(), Arc::new(buffer)))
}

fn allocate_buffer(
    decl: &BufferDecl,
    frames: usize,
    channels: usize,
    rate: Option<f32>,
    names: &Names,
    options: &CompileOptions,
) -> Res<(String, Arc<niminal_engine::Buffer>)> {
    let owner = decl
        .name
        .name
        .split_once("::")
        .map(|(n, _)| n)
        .unwrap_or("");
    if let Some(old) = options
        .buffers
        .get(&decl.name.name)
        .filter(|b| b.len() == frames && b.channels() == channels && b.sample_rate() == rate)
    {
        return Ok((decl.name.name.clone(), old.clone()));
    }
    let budget = options
        .buffer_budgets
        .get(owner)
        .copied()
        .unwrap_or(256 * 1024 * 1024);
    let retained = options
        .retained_buffer_bytes
        .get(owner)
        .copied()
        .unwrap_or(0);
    let used: usize = names
        .buffers
        .iter()
        .filter(|(n, (_, b))| {
            n.split_once("::").map(|(n, _)| n).unwrap_or("") == owner
                && !options.buffers.values().any(|old| Arc::ptr_eq(old, b))
        })
        .map(|(_, (_, b))| b.len() * b.channels() * 4)
        .sum();
    if frames
        .checked_mul(channels)
        .and_then(|n| n.checked_mul(4))
        .is_none_or(|n| n > budget.saturating_sub(retained.saturating_add(used)))
    {
        return Err(Diagnostic::new(
            "client buffer memory budget exceeded",
            decl.span,
        ));
    }
    Ok((
        decl.name.name.clone(),
        Arc::new(niminal_engine::Buffer::zeros_channels(
            frames, channels, rate,
        )),
    ))
}

/// The master layout from the project's `config`, mono if there is none.
fn master_layout(items: &[Item], default: Layout) -> Res<Layout> {
    let mut layout = None;
    let mut seen = false;
    for item in items {
        let Item::Config(config) = item else { continue };
        if seen {
            return Err(Diagnostic::new(
                "a project has only one `config`",
                config.span,
            ));
        }
        seen = true;
        for (key, value) in &config.entries {
            match key.name.as_str() {
                "channels" => layout = Some(layout_from_expr(value)?),
                other => {
                    let mut d = Diagnostic::new(format!("unknown setting `{other}`"), key.span);
                    d = d.with_help("the settings so far are: channels");
                    return Err(d);
                }
            }
        }
    }
    Ok(layout.unwrap_or(default))
}

/// Cents above the root (c) for the twelve notes of each named scale.
const NAMED_SCALES: [(&str, [f64; 12]); 4] = [
    (
        "equal",
        [
            0.0, 100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0, 800.0, 900.0, 1000.0, 1100.0,
        ],
    ),
    (
        "just",
        [
            0.0, 111.73, 203.91, 315.64, 386.31, 498.04, 590.22, 701.96, 813.69, 884.36, 1017.60,
            1088.27,
        ],
    ),
    (
        "pythagorean",
        [
            0.0, 113.69, 203.91, 294.13, 407.82, 498.04, 611.73, 701.96, 815.64, 905.87, 996.09,
            1109.78,
        ],
    ),
    (
        "meantone",
        [
            0.0, 76.05, 193.16, 310.26, 386.31, 503.42, 579.47, 696.58, 772.63, 889.74, 1006.84,
            1082.89,
        ],
    ),
];

/// `config { tuning: { a4: 432hz, scale: just, period: 1200ct } }`: how note names become frequencies.
/// `scale` is a name (`equal`, `just`, `pythagorean`, `meantone`), `cents(0, 112, ...)` or
/// `ratios(1, 16/15, ...)`, one entry per note number from c up to the period.
fn tuning_of(items: &[Item]) -> Res<Tuning> {
    let default = Tuning::default();
    let (mut a4, mut period, mut scale) = (default.a4, default.period, NAMED_SCALES[0].1.to_vec());
    for item in items {
        let Item::Config(config) = item else { continue };
        for (key, value) in &config.tuning {
            match key.name.as_str() {
                "a4" => match &value.kind {
                    ExprKind::Num {
                        value: v,
                        unit: Some(u),
                    } if (u == "hz" || u == "khz") && *v > 0.0 => {
                        a4 = if u == "khz" { v * 1000.0 } else { *v };
                    }
                    _ => {
                        return Err(Diagnostic::new("`a4` is a frequency", value.span)
                            .with_help("for example `a4: 432hz`"));
                    }
                },
                "period" => {
                    let cents = cents_of(value)?;
                    if cents <= 0.0 {
                        return Err(Diagnostic::new("`period` must be above 0ct", value.span));
                    }
                    period = cents;
                }
                "scale" => scale = scale_of(value)?,
                other => {
                    return Err(Diagnostic::new(
                        format!("unknown tuning setting `{other}`"),
                        key.span,
                    )
                    .with_help("the settings are: a4, scale, period"));
                }
            }
        }
    }
    Ok(Tuning::new(a4, period, &scale))
}

/// An interval written in cents (`702ct`) or semitones (`7st`), as cents.
fn cents_of(e: &Expr) -> Res<f64> {
    match &e.kind {
        ExprKind::Num {
            value,
            unit: Some(u),
        } if u == "ct" => Ok(*value),
        ExprKind::Num {
            value,
            unit: Some(u),
        } if u == "st" => Ok(*value * 100.0),
        ExprKind::Neg(inner) => cents_of(inner).map(|c| -c),
        _ => Err(Diagnostic::new(
            "expected an interval in cents, such as `702ct`",
            e.span,
        )),
    }
}

fn scale_of(e: &Expr) -> Res<Vec<f64>> {
    let check = |degrees: Vec<f64>, span| -> Res<Vec<f64>> {
        if degrees.is_empty() || degrees.len() > MAX_DEGREES {
            return Err(Diagnostic::new(
                format!("a scale has between 1 and {MAX_DEGREES} degrees"),
                span,
            ));
        }
        if degrees[0] != 0.0 {
            return Err(Diagnostic::new(
                "a scale starts at the root: its first degree is 0 cents (or a ratio of 1)",
                span,
            ));
        }
        Ok(degrees)
    };
    match &e.kind {
        ExprKind::Name(n) => NAMED_SCALES
            .iter()
            .find(|(name, _)| name == n)
            .map(|(_, cents)| cents.to_vec())
            .ok_or_else(|| {
                let names = NAMED_SCALES.map(|(name, _)| name);
                let mut d = Diagnostic::new(format!("unknown scale `{n}`"), e.span)
                    .with_help(format!("the named scales are: {}", names.join(", ")));
                if let Some(c) = closest(n, names) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                d
            }),
        ExprKind::Call { name, args } if name.name == "cents" => {
            // inside `cents(...)` a bare number is already in cents
            let cents = |a: &Arg| match &a.value.kind {
                ExprKind::Num { value, unit: None } => Ok(*value),
                _ => cents_of(&a.value),
            };
            check(args.iter().map(cents).collect::<Res<_>>()?, e.span)
        }
        ExprKind::Call { name, args } if name.name == "ratios" => {
            let ratio = |a: &Arg| -> Res<f64> {
                let value = match &a.value.kind {
                    ExprKind::Num { value, unit: None } => Some(*value),
                    ExprKind::Binary {
                        op: BinOp::Div,
                        lhs,
                        rhs,
                    } => match (&lhs.kind, &rhs.kind) {
                        (
                            ExprKind::Num {
                                value: n,
                                unit: None,
                            },
                            ExprKind::Num {
                                value: d,
                                unit: None,
                            },
                        ) if *d != 0.0 => Some(n / d),
                        _ => None,
                    },
                    _ => None,
                };
                match value {
                    Some(r) if r > 0.0 => Ok(1200.0 * r.log2()),
                    _ => Err(Diagnostic::new(
                        "expected a ratio above 0, such as `3/2`",
                        a.value.span,
                    )),
                }
            };
            check(args.iter().map(ratio).collect::<Res<_>>()?, e.span)
        }
        _ => Err(
            Diagnostic::new("a scale is a name, `cents(...)` or `ratios(...)`", e.span).with_help(
                "for example `scale: just` or `scale: ratios(1, 9/8, 5/4, 4/3, 3/2, 5/3, 15/8)`",
            ),
        ),
    }
}

/// Literal named arguments, checked against `instr`'s parameters.
fn literal_args(
    args: &[Arg],
    instr: &Instrument,
    sample_names: &[String],
    tempo: Tempo,
) -> Res<BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    for arg in args {
        let Some(arg_name) = &arg.name else {
            return Err(
                Diagnostic::new("name the arguments of a note", arg.value.span)
                    .with_help("for example `lead(freq: c4)`"),
            );
        };
        if let Some(paths) = instr.arrays.get(&arg_name.name) {
            fn leaves(e: &Expr, out: &mut Vec<Expr>) -> Res<()> {
                match &e.kind {
                    ExprKind::Array(items) => {
                        for item in items {
                            leaves(item, out)?;
                        }
                    }
                    _ => out.push(e.clone()),
                }
                Ok(())
            }
            let mut values = Vec::new();
            leaves(&arg.value, &mut values)?;
            if values.len() != paths.len() {
                return Err(Diagnostic::new(
                    format!("array `{}` needs {} values", arg_name.name, paths.len()),
                    arg.value.span,
                ));
            }
            for (path, e) in paths.iter().zip(values) {
                let value = literal_value(&e).map_err(|mut d| {
                    d.span = arg.value.span;
                    d
                })?;
                instr
                    .param_value(instr.param(path).unwrap().1, value, tempo)
                    .map_err(|e| arg_error(e, arg.value.span))?;
                if out.insert(path.clone(), value).is_some() {
                    return Err(Diagnostic::new(
                        "array argument given more than once",
                        arg_name.span,
                    ));
                }
            }
            continue;
        }
        let (_, param) = instr
            .param(&arg_name.name)
            .map_err(|e| arg_error(e, arg_name.span))?;
        let value = literal_arg_value(&arg.value, &arg_name.name, instr, sample_names)?;
        instr
            .param_value(param, value, tempo)
            .map_err(|e| arg_error(e, arg.value.span))?;
        if out.insert(arg_name.name.clone(), value).is_some() {
            return Err(Diagnostic::new(
                format!("argument `{}` given more than once", arg_name.name),
                arg_name.span,
            ));
        }
    }
    Ok(out)
}

/// A note argument: a literal, or a kit sample name for `sample:`.
fn literal_arg_value(
    e: &Expr,
    param: &str,
    instr: &Instrument,
    _sample_names: &[String],
) -> Res<Value> {
    if let Some(resources) = instr.resources.get(param) {
        let ExprKind::Name(name) = &e.kind else {
            return Err(Diagnostic::new(
                "buffer arguments are named shared buffers",
                e.span,
            ));
        };
        let index = resources
            .iter()
            .position(|n| n == name)
            .ok_or_else(|| Diagnostic::new(format!("unknown buffer `{name}`"), e.span))?;
        return Ok(Value::Num(index as f64));
    }
    if param == "sample"
        && let ExprKind::Name(name) = &e.kind
        && !instr.members.is_empty()
    {
        if let Some(local) = instr.members.iter().position(|m| m == name) {
            return Ok(Value::Num(local as f64));
        }
        let mut d = Diagnostic::new(
            format!("there is no sample `{name}` in the kit `{}`", instr.name),
            e.span,
        );
        if let Some(c) = closest(name, instr.members.iter().map(String::as_str)) {
            d = d.with_help(format!("did you mean `{c}`?"));
        }
        return Err(d);
    }
    literal_value(e)
}

/// A note's argument problem as a diagnostic at `span`.
fn arg_error(e: crate::program::ArgError, span: Span) -> Diagnostic {
    let d = Diagnostic::new(e.message, span);
    match e.help {
        Some(h) => d.with_help(h),
        None => d,
    }
}

fn literal_time(e: &Expr) -> Res<Time> {
    if let Ok((value, unit)) = folded_quantity(e) {
        return match unit.as_deref() {
            Some("sec") => Ok(Time::Seconds(value)),
            Some("ms") => Ok(Time::Seconds(value / 1000.0)),
            Some("beat" | "beats") => Ok(Time::Beats(value)),
            Some("bar" | "bars") => Ok(Time::Bars(value)),
            _ => Err(Diagnostic::new("expected a time", e.span)
                .with_help("for example `1beat`, `2bars` or `250ms`")),
        };
    }
    Err(Diagnostic::new("expected a time", e.span)
        .with_help("for example `1beat`, `2bars` or `250ms`"))
}

/// A number with a unit, including constant arithmetic such as `i * 1/2beat`.
fn folded_quantity(e: &Expr) -> Res<(f64, Option<String>)> {
    match &e.kind {
        ExprKind::Num { value, unit } => Ok((*value, unit.clone())),
        ExprKind::Neg(inner) => {
            let (v, u) = folded_quantity(inner)?;
            Ok((-v, u))
        }
        ExprKind::Binary { op, lhs, rhs } => {
            let ((l, lu), (r, ru)) = (folded_quantity(lhs)?, folded_quantity(rhs)?);
            let bad = || Diagnostic::new("can't combine these units", e.span);
            match op {
                BinOp::Add | BinOp::Sub if lu == ru => {
                    Ok((if *op == BinOp::Add { l + r } else { l - r }, lu))
                }
                BinOp::Mul if ru.is_none() => Ok((l * r, lu)),
                BinOp::Mul if lu.is_none() => Ok((l * r, ru)),
                BinOp::Div if ru.is_none() && r != 0.0 => Ok((l / r, lu)),
                BinOp::Div if lu == ru && r != 0.0 => Ok((l / r, None)),
                _ => Err(bad()),
            }
        }
        _ => Err(Diagnostic::new("expected a number", e.span)),
    }
}

/// Note arguments are plain values for now: a number with a unit, or a note name.
fn literal_value(e: &Expr) -> Res<Value> {
    let bad = || {
        Diagnostic::new("note arguments must be literal values", e.span)
            .with_help("for example `440hz`, `-6db`, `0.5` or `a3`")
    };
    match &e.kind {
        // Note args don't take `%`; value_of does (for pattern atoms).
        ExprKind::Num { value, unit } => match unit.as_deref() {
            Some("%") => Err(unknown_unit("%", e.span)),
            _ => value_of(*value, unit.as_deref(), e.span),
        },
        ExprKind::Name(n) => match n.parse::<Value>() {
            Ok(v @ Value::Note(_)) => Ok(v),
            _ => Err(bad()),
        },
        ExprKind::Neg(inner) => match literal_value(inner)? {
            Value::Note(_) | Value::Sample(_) => Err(bad()),
            v => Ok(negate(v)),
        },
        ExprKind::Binary { op, lhs, rhs } => {
            let (a, b) = (literal_value(lhs)?, literal_value(rhs)?);
            match op {
                BinOp::Add => fold_add(a, b, e.span),
                BinOp::Sub => fold_sub(a, b, e.span),
                BinOp::Mul => fold_mul(a, b, e.span),
                BinOp::Div => fold_div(a, b, e.span),
            }
        }
        _ => Err(bad()),
    }
}

fn fold_add(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Note(n), Value::Semitones(st)) | (Value::Semitones(st), Value::Note(n)) => {
            Ok(transpose(Value::Note(n), st))
        }
        (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a + b)),
        (Value::Db(a), Value::Db(b)) => Ok(Value::Db(a + b)),
        (Value::Semitones(a), Value::Semitones(b)) => Ok(Value::Semitones(a + b)),
        _ => Err(Diagnostic::new("can't add these note arguments", span)),
    }
}

fn fold_sub(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Note(n), Value::Semitones(st)) => {
            Ok(transpose(Value::Note(n), -st))
        }
        (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a - b)),
        (Value::Semitones(a), Value::Semitones(b)) => Ok(Value::Semitones(a - b)),
        _ => Err(Diagnostic::new("can't subtract these note arguments", span)),
    }
}

fn fold_mul(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a * b)),
        (Value::Semitones(s), Value::Num(n)) | (Value::Num(n), Value::Semitones(s)) => {
            Ok(Value::Semitones(s * n))
        }
        _ => Err(Diagnostic::new("can't multiply these note arguments", span)),
    }
}

fn fold_div(a: Value, b: Value, span: Span) -> Res<Value> {
    match (a, b) {
        (Value::Num(a), Value::Num(b)) if b != 0.0 => Ok(Value::Num(a / b)),
        _ => Err(Diagnostic::new("can't divide these note arguments", span)),
    }
}

fn owned_samples(options: &CompileOptions, name: &str) -> Samples {
    name.split_once("::")
        .and_then(|(owner, _)| options.namespace_samples.get(owner))
        .cloned()
        .unwrap_or_else(|| options.samples.clone())
}

fn scoped_group(context: &str, name: &str) -> String {
    if let Some((owner, _)) = context.split_once("::") {
        if name.starts_with(&format!("{owner}::")) {
            name.to_string()
        } else {
            format!("{owner}::{name}")
        }
    } else {
        name.to_string()
    }
}

/// A number with a unit as a score value: `440hz`, `-6db`, `2bars`.
pub fn value_of(value: f64, unit: Option<&str>, span: Span) -> Res<Value> {
    Ok(match unit {
        None => Value::Num(value),
        Some("hz") => Value::Hz(value),
        Some("khz") => Value::Hz(value * 1000.0),
        Some("db") => Value::Db(value),
        Some("sec") => Value::Seconds(value),
        Some("ms") => Value::Seconds(value / 1000.0),
        Some("beat" | "beats") => Value::Beats(value),
        Some("bar" | "bars") => Value::Bars(value),
        Some("deg") => Value::Degrees(value),
        Some("st") => Value::Semitones(value),
        Some("ct") => Value::Semitones(value / 100.0),
        Some("%") => Value::Num(value / 100.0),
        Some(u) => return Err(crate::lower::unknown_unit(u, span)),
    })
}

/// The same quantity the other way round. A note or a sample keeps its sign.
pub fn negate(v: Value) -> Value {
    match v {
        Value::Num(n) => Value::Num(-n),
        Value::Hz(n) => Value::Hz(-n),
        Value::Db(n) => Value::Db(-n),
        Value::Seconds(n) => Value::Seconds(-n),
        Value::Beats(n) => Value::Beats(-n),
        Value::Bars(n) => Value::Bars(-n),
        Value::Semitones(n) => Value::Semitones(-n),
        Value::Degrees(n) => Value::Degrees(-n),
        keep @ (Value::Note(_) | Value::Sample(_)) => keep,
    }
}

/// Move a pitch by `st` semitones. Notes stay notes when the interval is whole.
pub fn transpose(v: Value, st: f64) -> Value {
    match v {
        Value::Note(m) if st.fract() == 0.0 => Value::Note(m + st as i32),
        Value::Note(_) | Value::Hz(_) => {
            let hz = v.as_hz().expect("notes and frequencies have a frequency");
            Value::Hz(hz * 2f64.powf(st / 12.0))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> Program {
        compile(src).unwrap_or_else(|errs| {
            panic!(
                "{}",
                errs.iter().map(|d| d.render("t", src)).collect::<String>()
            );
        })
    }

    fn errors(src: &str) -> Vec<Diagnostic> {
        compile(src).err().expect("expected compile errors")
    }

    fn first_error(src: &str) -> Diagnostic {
        errors(src).remove(0)
    }

    fn msg(src: &str) -> String {
        first_error(src).message
    }

    fn help(src: &str) -> Option<String> {
        first_error(src).help
    }


    #[test]
    fn arithmetic_with_units() {
        ok(
            "instr a(freq: hz, bright: 0..1 = 0.5) { osc(saw, freq * 2).lpf(cutoff: freq * (2 + bright * 6)) }",
        );
        ok("instr a(freq: hz, amp: db = -6db) { osc(sine, freq).gain(amp - 6db).gain(0.5) }");
        ok("instr a(freq: hz) { osc(sine, freq + 3hz - 1hz) }");
        ok("instr a(freq: hz) { osc(sine, freq / 2) }");
        ok("instr a() { 0.5 }");
    }

    #[test]
    fn bare_literal_in_a_unit_slot_gets_a_quick_fix() {
        let src = "instr a(f: hz) { osc(saw, f).lpf(cutoff: 800) }";
        let d = first_error(src);
        assert_eq!(d.message, "`cutoff` expects hz, found a plain number");
        assert_eq!(d.help.as_deref(), Some("did you mean `800hz`?"));
        assert_eq!(&src[d.span.start..d.span.end], "800");

        let d = first_error("instr a() { osc(saw, 440) }");
        assert_eq!(d.help.as_deref(), Some("did you mean `440hz`?"));
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f).lpf(cutoff: sine) }"),
            "`sine` is a waveform, not a value"
        );
    }

    #[test]
    fn wrong_units_are_errors() {
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f).lpf(cutoff: 6db) }"),
            "`cutoff` expects hz, found db"
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f).gain(440hz) }"),
            "`gain` expects db or a plain factor, found hz"
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f + 6db) }"),
            "can't add hz and db"
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f * f) }"),
            "can't multiply hz and hz"
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f + 1) }"),
            "can't add hz and a plain number"
        );
        assert_eq!(
            msg("instr a(f: hz) { f }"),
            "an instrument outputs a plain signal, but this is hz"
        );
        assert_eq!(
            msg("instr a(f: hz = 6db) { osc(saw, f) }"),
            "`f` expects hz, found db"
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f).gain(f * 2db) }"),
            "can't multiply hz and db"
        );
        // dividing by zero gives 0, as it does while playing
        ok("instr a(f: hz) { osc(saw, f / 0) }");
    }

    #[test]
    fn name_errors_suggest_fixes() {
        let d = first_error("instr a(freq: hz) { osc(saw, frq) }");
        assert_eq!(d.message, "`frq` is not defined");
        assert_eq!(d.help.as_deref(), Some("did you mean `freq`?"));
        assert_eq!(
            help("instr a(f: hz) { osc(saw, f).lpf(cuttoff: 1khz) }").as_deref(),
            Some("did you mean `cutoff`?")
        );
        assert_eq!(
            help("instr a(f: hz) { osc(saw, f).lfp(cutoff: 1khz) }").as_deref(),
            Some("did you mean `lpf`?")
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f).wobble() }"),
            "unknown function `wobble`"
        );
        assert_eq!(
            msg("instr a(f: hz) { saw }"),
            "`saw` is a waveform, not a value"
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(wobble, f) }"),
            "expected a waveform or a table"
        );
        assert_eq!(
            msg("instr a(f: hertz) { osc(saw, f) }"),
            "unknown type `hertz`"
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, 440herz) }"),
            "unknown unit `herz`"
        );
    }

    #[test]
    fn argument_binding_rules() {
        // leading required parameters may be given in order; the rest are named
        ok("instr a(f: hz) { osc(saw, f).lpf(2khz) }");
        let d = first_error("instr a(f: hz) { osc(saw, f).lpf(2khz, 0.3) }");
        assert_eq!(d.message, "`lpf` takes its other arguments by name");
        assert_eq!(d.help.as_deref(), Some("write `res: ...`"));
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f).lpf(res: 0.1) }"),
            "`lpf` needs an argument `cutoff`"
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw) }"),
            "`osc` needs an argument `freq`"
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f, f) }"),
            "`osc` takes its other arguments by name"
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, freq: f, freq: f) }"),
            "`osc` argument `freq` given more than once"
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f, freq: f) }"),
            "`osc` argument `freq` given more than once"
        );
        // the receiver fills the first parameter, so naming it as well clashes
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f).lpf(x: f, cutoff: 1hz) }"),
            "`lpf` argument `x` given more than once"
        );
    }

    #[test]
    fn instrument_shape_errors() {
        assert_eq!(msg("instr a() { }"), "instrument `a` is empty");
        assert_eq!(
            msg("instr a() { x = 0.5 }"),
            "an instrument must end with the signal it outputs"
        );
        assert_eq!(msg("instr a() { 0.5\n0.5 }"), "this value is never used");
        assert_eq!(
            msg("instr a(f: hz, f: hz) { 0.5 }"),
            "parameter `f` is declared twice"
        );
        assert_eq!(
            msg("instr a() { 0.5 }\ninstr a() { 0.5 }"),
            "`a` is defined twice"
        );
        assert_eq!(
            msg("instr a(b: 0..1 = 2) { b }"),
            "the default for `b` is outside 0..1"
        );
        assert_eq!(
            msg("instr a(b: 1..0) { b }"),
            "the lower end of a range must be below the upper end"
        );
    }

    #[test]
    fn envelope_unit_rules() {
        ok("instr a() { osc(sine, env[200hz 10ms 6khz 300ms.exp 900hz]) }");
        assert!(
            msg("instr a() { osc(sine, env[0 10ms 6khz]) }")
                .starts_with("envelope levels must all have the same unit")
        );
        assert_eq!(
            msg("instr a() { env[-6db 10ms 0db] }"),
            "db levels in envelopes aren't supported yet"
        );
    }


    const SPEC_OPCODES: &str = "
fn one_pole(x, cutoff: hz) {
  state y = 0.0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  y
}

fn drive(x, amount: 0..1 = 0.5) {
  (x * (1 + amount * 9)).tanh
}

fn dc_block(x) {
  state x1 = 0.0
  state y1 = 0.0
  y = x - x1 + 0.995 * y1
  x1 = x
  y1 = y
  y
}

fn fold(x, amount: 0..1 = 0.5) {
  (x * (1 + amount * 8)).sin
}
";

    #[test]
    fn the_specs_example_functions_compile_and_can_be_used() {
        ok(&format!(
            "{SPEC_OPCODES}
instr a(freq: hz) {{ osc(saw, freq).drive(amount: 0.3).one_pole(cutoff: 800hz) }}
instr b(freq: hz) {{ osc(sine, freq).fold(amount: env[0 2sec 1 2sec 0]).dc_block }}
instr c(freq: hz) {{ drive(osc(saw, freq)) }}"
        ));
    }

    #[test]
    fn function_arguments_are_checked_like_any_other() {
        let with = |call: &str| format!("{SPEC_OPCODES}\ninstr a(freq: hz) {{ {call} }}");
        assert_eq!(
            msg(&with("osc(saw, freq).one_pole(cutoff: 800)")),
            "`cutoff` expects hz, found a plain number"
        );
        assert_eq!(
            help(&with("osc(saw, freq).one_pole(cutoff: 800)")).as_deref(),
            Some("did you mean `800hz`?")
        );
        assert_eq!(
            msg(&with("osc(saw, freq).one_pole()")),
            "`one_pole` needs an argument `cutoff`"
        );
        assert_eq!(
            msg(&with("osc(saw, freq).drive(amont: 0.3)")),
            "`drive` has no argument named `amont`"
        );
        assert_eq!(
            msg(&with("osc(saw, freq).drive(0.3)")),
            "`drive` takes its other arguments by name"
        );
        ok(&with("osc(saw, freq).drive"));
        ok(&with("osc(saw, freq).one_pole(800hz)"));
    }

    #[test]
    fn function_body_errors() {
        let body =
            |b: &str| format!("fn f(x: num, amount: 0..1 = 0.5, cutoff: hz = 100hz) {{ {b} }}");
        let state = |b: &str| body(&format!("state s = 0.0\n{b}"));
        assert_eq!(
            msg(&body("x * undefined_thing")),
            "`undefined_thing` is not defined"
        );
        assert_eq!(
            msg(&body("exp(cutoff)")),
            "`exp` expects a plain number, found hz"
        );
        assert_eq!(msg(&body("x + cutoff")), "can't add a plain number and hz");
        // a function can give any unit
        ok(&body("cutoff"));
        ok(&state("x.lpf(cutoff: 100hz) + s"));
        assert_eq!(
            msg(&state("x.pan(azimuth: 10deg) + s")),
            "`pan` can't be used inside a function with state"
        );
        assert_eq!(msg(&state("x.exxp + s")), "unknown function `exxp`");
        assert_eq!(
            help(&state("x.exxp + s")).as_deref(),
            Some("did you mean `exp`?")
        );
        assert_eq!(msg(&state("exp(x, x) + s")), "`exp` takes one value");
        assert_eq!(
            msg(&state("env[0 1sec 1] + s")),
            "envelopes can't be used inside a function with state"
        );
        assert_eq!(msg(&body("")), "`f` must end with the value it gives");
        // a state starts at a value known at note start, here the input's first sample
        ok(&body("state s = x\ns"));
        assert_eq!(
            msg(&state("s = cutoff\ns")),
            "`s` holds a plain number, but this is hz"
        );
        assert_eq!(msg(&body("state x = 0.0\nx")), "`x` is already defined");
        assert_eq!(msg(&state("y = 1")), "`f` must end with the value it gives");
        assert_eq!(msg(&body("x\nx")), "this value is never used");
        ok("fn f(x) -> hz { x * 2 }\ninstr a(freq: hz) { osc(sine, f(freq * 0.5)) }");
        assert_eq!(
            msg("fn f(x) -> hz { x * 2 }\ninstr a(freq: hz) { osc(sine, freq * f(0.5)) }"),
            "`f` gives a plain number, but its result is declared as hz"
        );
    }

    #[test]
    fn function_signature_errors() {
        assert_eq!(
            msg("fn lpf(x) { x }"),
            "`lpf` is a built-in function, so it can't be defined again"
        );
        assert_eq!(msg("fn f(x) { x }\nfn f(x) { x }"), "`f` is defined twice");
        assert_eq!(msg("fn f(x, x) { x }"), "parameter `x` is declared twice");
        assert_eq!(
            msg("fn f(x: num, a: 0..1 = 3) { x }\ninstr i(freq: hz) { osc(saw, freq).f }"),
            "`a` is between 0 and 1, but this is 3"
        );
        assert_eq!(msg("fn f(x, a: furlongs) { x }"), "unknown type `furlongs`");
        assert_eq!(msg("opcode f(x) { x }"), "`opcode` is now `fn`");
    }

    #[test]
    fn state_and_untyped_parameters_belong_to_the_right_definitions() {
        assert_eq!(
            msg("instr a() { state y = 0.0\n0.5 }"),
            "`state` can only be used inside a `fn`"
        );
        assert_eq!(
            msg("instr a(freq) { osc(saw, freq) }"),
            "parameter `freq` needs a type"
        );
    }

    #[test]
    fn a_broken_function_is_reported_once_even_if_it_is_used() {
        let errs = errors(
            "
fn bad(x) { x + undefined_thing }
instr a(f: hz) { osc(saw, f).bad }
",
        );
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert_eq!(errs[0].message, "`undefined_thing` is not defined");
        // even when nothing calls it
        assert_eq!(
            msg("fn bad(x) { x + undefined_thing }"),
            "`undefined_thing` is not defined"
        );
    }

    #[test]
    fn rates_on_parameters_are_checked() {
        // `const` needs a value known while compiling
        ok(
            "fn rep(x, n: const int = 2) { (for i in 0..<n { x }).sum }\ninstr a(f: hz) { osc(saw, f).rep(n: 3) }",
        );
        assert_eq!(
            msg(
                "fn rep(x, n: const int = 2) { x * n }\ninstr a(f: hz) { osc(saw, f).rep(n: f / 1hz) }"
            ),
            "`n` must be known while compiling"
        );
        assert_eq!(
            msg(
                "fn rep(x, n: const int = 2) { x * n }\ninstr a(f: hz) { osc(saw, f).rep(n: 2.5) }"
            ),
            "`n` expects a whole number, found 2.5"
        );
        // Structural parameters need defaults; scalar parameters retain their rate.
        assert_eq!(
            msg("instr a(f: const hz) { osc(saw, f) }"),
            "structural parameter `f` needs a default"
        );
        ok("instr a(f: init hz, cut: control hz = 1khz) { osc(saw, f).lpf(cut) }");
        assert_eq!(
            msg("fn f(mode: init (a | b) = a) { 1 }"),
            "a choice is always known while compiling"
        );
    }

    #[test]
    fn choices_and_arrays_specialize_functions_and_instruments() {
        ok("instr a(mode: (x | y) = x) { if mode == x { 0.5 } else { 0.25 } }");
        ok(
            "instr a(freq: hz, partials: [num] = [1, 0.5], count: const int = 2) { (for i in 0..<count { osc(sine, freq * (i + 1)) * partials[i] }).sum }\ninstr b(freq: hz) { a(freq, partials: [1, 0.2, 0.1], count: 3) }",
        );
        ok(
            "fn sine(f: hz) { osc(sine, f) }\ninstr a(freq: hz, make: fn = sine) { make(freq) }\ninstr b(freq: hz) { a(freq, make: fn(f) { osc(saw, f) }) }",
        );
        assert_eq!(
            msg("fn f(m: (a | b) = c) { 1 }"),
            "the default for `m` is one of a, b"
        );
        assert_eq!(
            msg("fn f(m: (a) = a) { 1 }"),
            "a choice lists at least two names"
        );
        ok("fn f(xs: [num]) { xs.sum }\ninstr a() { f([0.1, 0.2]) * osc(sine, 100hz) }");
        ok("fn f(xs: [num]) { xs.sum }\ninstr a() { f(0.5) * osc(sine, 100hz) }");
    }

    #[test]
    fn functions_have_no_side_effects() {
        assert_eq!(
            msg("bus b\nfn f(x) { b += x\n x }\ninstr a(f: hz) { osc(saw, f).f }"),
            "a function can't send to the bus `b`"
        );
        assert_eq!(
            msg("fn f(x: num) { x * note.time.num }"),
            "`note` is only available in an instrument"
        );
        // a function's locals may share a name with an instrument, which it can't read
        ok(
            "instr y(f: hz) { osc(saw, f) }\nfn g(x) {\n y = x * 2\n y\n}\ninstr a(f: hz) { osc(saw, f).g }",
        );
    }

    #[test]
    fn statements_in_blocks() {
        ok("instr a(f: hz) {\n g = 1\n if f > 1khz { g = 0.5 }\n osc(saw, f) * g\n}");
        ok("instr a(f: hz) {\n if f > 1khz { out = osc(saw, f) } else { out = osc(sine, f) }\n}");
        assert_eq!(
            msg("bus b\ninstr a(f: hz) {\n if f > 1khz { b += osc(saw, f) }\n osc(saw, f)\n}"),
            "a send inside an `if` that changes while playing would always be made"
        );
        // a name first bound in a block stays in it
        assert!(
            msg("instr a(f: hz) {\n if f > 1khz { g = 0.5 }\n osc(saw, f) * g\n}")
                .contains("`g` is not defined")
        );
        assert_eq!(
            msg("fn f(x) {\n if x > 0 { state s = 0 }\n x\n}\ninstr a(f: hz) { osc(saw, f).f }"),
            "state is declared at the top of a function, not inside a block"
        );
        assert_eq!(
            msg("instr a() { if 1 > 0 { 0.5 } }"),
            "an `if` used as a value needs an `else`"
        );
    }

    #[test]
    fn a_tail_is_a_time() {
        ok("instr a(f: hz) with(tail: 2sec) { osc(saw, f) }");
        assert_eq!(
            msg("instr a(f: hz) with(tail: 2) { osc(saw, f) }"),
            "`tail` is a time from 0 to 600sec, such as `2sec`"
        );
    }

    #[test]
    fn constants() {
        let p = ok(
            "const k = 2\nconst ratios = (1..3).map(it * k)\ninstr a(f: hz) { osc(saw, f * ratios).sum * 0.1 }",
        );
        assert_eq!(p.instruments[0].graph.channels(), 1);
        assert_eq!(msg("const k = 1\nconst k = 2"), "`k` is defined twice");
        assert_eq!(
            msg("const k = 1\ninstr a(f: hz) { k = 2\n osc(saw, f) }"),
            "`k` is already the name of a constant, so it can't be used for a local"
        );
    }

    #[test]
    fn db_gains_apply_to_plain_values() {
        ok("instr a(f: hz, amp: db = -6db) { osc(saw, f) * amp }");
        ok("fn g(x, amp: db = -6db) { x * amp }\ninstr a(f: hz) { osc(saw, f).g(amp: -3db) }");
    }

    #[test]
    fn routing_a_track_to_a_bus() {
        let p = ok("
bus fx
track a { instrument = i\n out = it.to(fx) }
track b { out = fx }
instr i(f: hz) { osc(sine, f) }");
        assert_eq!(p.tracks[1].def.route, niminal_engine::Route::Bus(0));
        assert_eq!(
            p.tracks[2].def.inputs,
            [niminal_engine::ChainInput::Bus { bus: 0, channel: 0 }]
        );
    }

    #[test]
    fn out_and_it_rules() {
        // an instrument either sets out or ends with a value, not both
        assert_eq!(
            msg("instr a(f: hz) { out = osc(saw, f)\nosc(sine, f) }"),
            "this instrument sets `out`, so its last line can't also be a value"
        );
        ok("instr a(f: hz) { out = osc(saw, f)\nout += osc(sine, f / 2) }");
        assert_eq!(
            msg("instr a(f: hz) { out += out\nout }"),
            "`out` has no value yet"
        );
        assert_eq!(
            msg("instr a(f: hz) { out = f }"),
            "`out` carries a plain signal, but this is hz"
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, it) }"),
            "`it` is only available inside a track, or a function given to `map`"
        );
        assert_eq!(
            msg("instr a(f: hz) { it = 1 }"),
            "`it` is an input and can't be assigned"
        );
        assert_eq!(
            msg("instr a(f: hz) { instrument = 1\n0.5 }"),
            "`instrument` is only available in a track"
        );
        // only sends: silent but valid
        ok("bus b\ninstr a(f: hz) { b += osc(saw, f) }");
    }

    #[test]
    fn bus_rules() {
        let with = |body: &str| format!("bus space\n{body}");
        assert_eq!(
            msg(&with("instr a(f: hz) { osc(saw, space) }")),
            "`space` is a bus, and only a track can read a bus"
        );
        assert_eq!(
            help(&with("instr a(f: hz) { osc(saw, space) }")).as_deref(),
            Some("an instrument can send to it with `space += ...`")
        );
        assert_eq!(
            msg(&with(
                "instr a(f: hz) { x = osc(saw, f)\nx += osc(saw, f)\nx }"
            )),
            "`+=` can only add to `out` or a bus, not `x`"
        );
        assert_eq!(
            help(&with(
                "instr a(f: hz) { x = osc(saw, f)\nx += osc(saw, f)\nx }"
            ))
            .as_deref(),
            Some("to change a local, write `x = x + ...`")
        );
        assert_eq!(
            help(&with("instr a(f: hz) { spaec += osc(saw, f) }")).as_deref(),
            Some("did you mean the bus `space`?")
        );
        assert_eq!(
            msg(&with("instr a(f: hz) { space += f\n0.5 }")),
            "`space` carries a plain signal, but this is hz"
        );
        assert_eq!(msg("bus a\nbus a"), "bus `a` is defined twice");
        assert_eq!(msg("bus a: wobble"), "unknown channel layout `wobble`");
        assert_eq!(
            msg("bus a: surround(9.1)"),
            "unsupported surround layout `9.1`"
        );
        ok("bus a: mono\nbus b: stereo\nbus c: surround(5.1)\nbus d: channels(3)");
    }

    #[test]
    fn locals_cannot_shadow_top_level_names() {
        let with = |body: &str| format!("bus space\ninstr other(f: hz) {{ osc(sine, f) }}\n{body}");
        assert_eq!(
            msg(&with("instr a(f: hz) { space = 1\nosc(saw, f) }")),
            "`space` is already the name of a bus, so it can't be used for a local"
        );
        assert_eq!(
            msg(&with("instr a(other: hz) { osc(saw, other) }")),
            "`other` is already the name of an instrument, so it can't be used for a local"
        );
        assert_eq!(
            msg(&with("instr a(f: hz) { space = 1 }")),
            "`space` is already the name of a bus, so it can't be used for a local"
        );
        assert_eq!(
            msg("bus x\ninstr x(f: hz) { osc(saw, f) }"),
            "`x` is already the name of a bus"
        );
    }

    #[test]
    fn track_errors() {
        let base = "bus space\ninstr i(f: hz, b: 0..1 = 0.5) { osc(saw, f) }\n";
        let track = |body: &str| format!("{base}track t {{ {body} }}");
        assert_eq!(msg(&track("")), "track `t` does nothing");
        assert_eq!(msg(&track("x = 1")), "track `t` never sets `out`");
        ok(&track("out = it")); // valid, though silent: nothing feeds `it`
        assert_eq!(
            msg(&track("instrument = i\ninstrument = i")),
            "a track has only one instrument"
        );
        assert_eq!(msg(&track("instrument = j")), "no instrument named `j`");
        assert_eq!(
            msg(&track("instrument = i(q: 1)")),
            "`i` has no parameter `q`"
        );
        assert_eq!(
            msg(&track("instrument = i(b: 2)")),
            "argument `b` must be between 0 and 1, got 2"
        );
        assert_eq!(
            msg(&track("instrument = i\n0.5")),
            "a track makes sound through `out`"
        );
        assert_eq!(
            msg(&track("instrument = i\nout = it.to(spce)")),
            "`spce` is not defined"
        );
        assert_eq!(
            msg(&track("instrument = i\nout = it.to(1)")),
            "`to` takes the name of a bus"
        );
        assert_eq!(
            msg(&track("instrument = i\nout = it.gain(1).to(space).gain(1)")),
            "`to` can only end the line that sets a track's `out`"
        );
        assert_eq!(
            msg(&format!(
                "{base}track t {{ instrument = i }}\ntrack t {{ instrument = i }}"
            )),
            "track `t` is defined twice"
        );
        // a track with an instrument and nothing else just plays it
        ok(&track("instrument = i"));
    }

    #[test]
    fn feedback_between_tracks_is_an_error_naming_them() {
        let src = "
bus x
bus y
track a { out = x.lpf(cutoff: 1khz).to(y) }
track b { out = y.lpf(cutoff: 1khz).to(x) }
";
        let d = first_error(src);
        assert!(
            d.message == "tracks feed back through buses or buffers: a -> b -> a"
                || d.message == "tracks feed back through buses or buffers: b -> a -> b",
            "{}",
            d.message
        );
        assert!(d.help.unwrap().contains("`late`"));

        // a late send breaks the loop
        ok(
            "bus x\nbus y\ntrack a { out = x.lpf(cutoff: 1khz).to(y) }\ntrack b { out = y\n x += late(out) }",
        );
        // outside a send, `prev` is one sample earlier
        ok("bus x\ntrack a { out = prev(x) }");
        let d = first_error("bus x\ntrack a { out = x.to(x) }");
        assert_eq!(
            d.message,
            "tracks feed back through buses or buffers: a -> a"
        );
    }

    #[test]
    fn a_broken_instrument_does_not_cascade_into_its_tracks_and_notes() {
        let errs = errors(
            "
instr bad(f: hz) { osc(saw, 440) }
track t { instrument = bad }
",
        );
        assert_eq!(errs.len(), 1, "{errs:?}");
    }

    #[test]
    fn a_changing_delay_time_needs_a_max() {
        let e = msg("instr a(t: sec) { osc(saw, 100hz).delay(time: t) }");
        assert_eq!(
            e,
            "`delay` needs a constant `time`, or a `max:` that bounds a changing one"
        );
        ok("instr a(t: sec) { osc(saw, 100hz).delay(time: t, max: 2sec) }");
        assert_eq!(
            msg("instr a(t: sec) { osc(saw, 100hz).delay(time: t, max: 31sec) }"),
            "a delay can be at most 30 seconds long"
        );
        assert_eq!(
            msg("instr a() { osc(saw, 100hz).delay(time: 40sec) }"),
            "a delay can be at most 30 seconds long"
        );
        assert_eq!(
            msg("instr a(t: sec) { osc(saw, 100hz).delay(time: t, max: 5) }"),
            "`max` expects a time, found a plain number"
        );
    }

    #[test]
    fn config_sets_the_master_layout_and_instruments_follow_it() {
        let src = |config: &str| format!("{config}\ninstr a(f: hz) {{ osc(saw, f) }}");
        assert_eq!(ok(&src("")).master, Layout::Mono);
        for (config, layout, channels) in [
            ("config { channels: stereo }", Layout::Stereo, 2),
            ("config { channels: quad }", Layout::Quad, 4),
            ("config { channels: surround(5.1) }", Layout::Surround51, 6),
            ("config { channels: surround(7.1) }", Layout::Surround71, 8),
        ] {
            let p = ok(&src(config));
            assert_eq!(p.master, layout);
            assert_eq!(p.instruments[0].graph.channels(), channels, "{config}");
        }
        // an unnamed layout has no automatic conversion from mono, but a list of that many works
        assert_eq!(
            msg(&src("config { channels: channels(3) }")),
            "can't convert mono to ch[3] automatically"
        );
        let three = ok(
            "config { channels: channels(3) }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f), osc(saw, f)] }",
        );
        assert_eq!(three.instruments[0].graph.channels(), 3);
        assert_eq!(
            msg(&src("config { channels: wobble }")),
            "unknown channel layout `wobble`"
        );
        assert_eq!(msg(&src("config { speed: 2 }")), "unknown setting `speed`");
        assert_eq!(
            msg(&src(
                "config { channels: stereo }\nconfig { channels: mono }"
            )),
            "a project has only one `config`"
        );
        assert_eq!(
            msg(&src("config { channels: channels(40) }")),
            "`channels` takes a count from 1 to 16"
        );
    }

    #[test]
    fn pan_places_a_mono_signal_in_the_master_layout() {
        let p = ok(
            "config { channels: surround(5.1) }\ninstr a(f: hz) { osc(saw, f).pan(azimuth: 20deg, spread: 0.3) }",
        );
        assert_eq!(p.instruments[0].graph.channels(), 6);
        ok(
            "config { channels: stereo }\ninstr a(f: hz, az: deg = 10deg) { osc(saw, f).pan(azimuth: az) }",
        );

        let d = first_error(
            "config { channels: stereo }\ninstr a(f: hz) { osc(saw, f).pan(azimuth: 20) }",
        );
        assert_eq!(
            d.message,
            "`azimuth` expects an angle, found a plain number"
        );
        assert_eq!(d.help.as_deref(), Some("did you mean `20deg`?"));
        assert_eq!(
            msg("config { channels: stereo }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f)].pan }"),
            "`pan` takes a one-channel signal, but `x` has 2"
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f).pan(azimuth: 5hz) }"),
            "`azimuth` expects an angle, found hz"
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f) * 20deg }"),
            "an instrument outputs a plain signal, but this is an angle"
        );
    }

    #[test]
    fn angles_follow_the_unit_rules() {
        ok(
            "config { channels: stereo }\ninstr a(f: hz) { osc(saw, f).pan(azimuth: 20deg * 2 - 5deg) }",
        );
        assert_eq!(
            msg(
                "config { channels: stereo }\ninstr a(f: hz) { osc(saw, f).pan(azimuth: 20deg + 1hz) }"
            ),
            "can't add an angle and hz"
        );
    }

    #[test]
    fn channel_lists_and_destructuring() {
        let stereo =
            |body: &str| format!("config {{ channels: stereo }}\ninstr a(f: hz) {{ {body} }}");
        let p = ok(&stereo("[osc(saw, f), osc(sine, f)]"));
        assert_eq!(p.instruments[0].graph.channels(), 2);

        ok(&stereo(
            "[l, r] = osc(saw, f).pan\nmid = (l + r) * 0.5\n[mid + l, mid - r]",
        ));
        // an opcode applied to a stereo signal runs once per channel
        ok(&stereo(
            "[osc(saw, f), osc(saw, f)].lpf(cutoff: 800hz).delay(time: 100ms).reverb",
        ));
        // different cutoffs per channel
        ok(&stereo("osc(saw, f).pan.lpf(cutoff: [800hz, 1200hz])"));

        assert_eq!(
            msg(&stereo("[l, r, c] = osc(saw, f).pan\nl")),
            "this signal has 2 channel(s), but 3 name(s) are given"
        );
        assert_eq!(
            msg(&stereo("[osc(saw, f).pan, osc(saw, f)]")),
            "a channel array takes one channel per entry, but this has 2"
        );
        assert_eq!(
            msg(&stereo("[f, f]")),
            "an instrument outputs a plain signal, but this is hz"
        );
        assert_eq!(
            msg(&stereo("[osc(saw, f), f]")),
            "the entries of a channel array must have the same unit: this is hz, the first was a plain number"
        );
        assert_eq!(
            msg(&stereo(
                "[osc(saw, f), osc(saw, f)] + [osc(saw, f), osc(saw, f), osc(saw, f)]"
            )),
            "can't combine an array of 2 with an array of 3"
        );
        assert_eq!(
            msg(&stereo("osc(saw, [f, f]).lpf(cutoff: [1khz, 2khz, 3khz])")),
            "opcode array arguments must have matching shapes"
        );
        assert_eq!(
            msg(&stereo("[l, r] = osc(saw, f)\nl")),
            "this signal has 1 channel(s), but 2 name(s) are given"
        );
    }

    #[test]
    fn lists_map_and_sum() {
        let mono = |body: &str| format!("instr a(f: hz) {{ {body} }}");
        // a supersaw: seven detuned oscillators added into one signal
        let p = ok(&mono(
            "(0..7).map(fn(i) { osc(saw, f * (1 + (i - 3) * 0.004)) }).sum * 0.2",
        ));
        assert_eq!(p.instruments[0].graph.channels(), 1);
        // a list literal maps too, and the parameter can be used in a unit
        ok(&mono(
            "[1, 2, 3].map(fn(h) { osc(sine, f * h) * (1 / h) }).sum",
        ));
        // mapping to a list keeps it a list
        let p = ok(
            "config { channels: stereo }\ninstr a(f: hz) { [1, 1.01].map(fn(r) { osc(saw, f * r) }) }",
        );
        assert_eq!(p.instruments[0].graph.channels(), 2);

        assert_eq!(
            msg(&mono("(3..<3).map(fn(i) { osc(saw, f) }).sum")),
            "this array is empty"
        );
        // `..` includes its end
        ok(&mono("(1..8).map(fn(n) { osc(sine, f * n) / n }).sum"));
        // a list is not limited by the channel count until it becomes a signal
        ok(&mono(
            "(0..40).map(fn(i) { osc(saw, f * (1 + i * 0.001)) }).sum",
        ));
        assert_eq!(
            msg(&mono("(0..17).map(fn(i) { osc(saw, f) })")),
            "audio output supports at most 16 channels"
        );
        // an expression of `it` is a function of one parameter
        ok(&mono("[1, 2].map(osc(saw, f * it)).sum"));
        assert_eq!(msg(&mono("fn(x) { x }")), "a function is not a signal");
        assert_eq!(
            msg(&mono("[1, 2].map(fn(f) { osc(saw, f) }).sum")),
            "`f` is already in use, so it can't be this function's parameter"
        );
        // the parameter is not visible after the map
        assert!(msg(&mono("[1].map(fn(x) { x }).sum + x")).contains("x"));
        // functions take and give lists
        ok(
            "fn spread(f, n: const int = 3) { for i in 0..<n { f * (1 + i * 0.01) } }\ninstr a(f: hz) { osc(saw, spread(f)).sum }",
        );
    }

    #[test]
    fn indexing_and_for_loops() {
        let mono = |body: &str| format!("instr a(f: hz) {{ {body} }}");
        // `for` is `map` with the function written inline
        let p = ok(&mono(
            "(for i in 0..7 { osc(saw, f * (1 + (i - 3) * 0.004)) }).sum * 0.2",
        ));
        assert_eq!(p.instruments[0].graph.channels(), 1);
        ok(&mono("for h in [1, 2, 3] { osc(sine, f * h) }.sum"));
        // indexing picks one entry; the index can come from a loop
        ok(&mono("bank = [osc(saw, f), osc(sine, f)]\nbank[1] * 0.5"));
        ok(&mono(
            "bank = [osc(saw, f), osc(sine, f)]\n(for i in 0..2 { bank[i] * (i + 1) }).sum",
        ));
        // a unit survives an index
        ok(&mono("osc(saw, [f, f * 2][1])"));

        // indexes wrap, so they never fail
        ok(&mono("[osc(saw, f), osc(sine, f)][2]"));
        ok(&mono("[osc(saw, f), osc(sine, f)][-1]"));
        // an index that changes switches between the entries
        ok(&mono("[osc(saw, f), osc(sine, f)][osc(square, 2hz).abs]"));
        ok(&mono("osc(saw, [100hz, 200hz, 300hz][osc(saw, 1hz) * 3])"));
        assert_eq!(
            msg(&mono("[osc(saw, f)][f]")),
            "an index is a plain number, found hz"
        );
        ok("fn o(x: num) { x[0] }");
    }

    #[test]
    fn lists_are_their_own_values() {
        let mono = |body: &str| format!("instr a(f: hz) {{ {body} }}");
        // lists of lists, `len`, and arithmetic on lists
        ok(&mono(
            "rows = for r in 0..3 { for c in 0..4 { osc(saw, f * (1 + (r * 4 + c) * 0.001)) } }\nrows[1][2] + rows[2].sum * (1 / rows.len)",
        ));
        ok(&mono("([1, 2, 3] * 2).sum * 0.1 + osc(saw, f)"));
        // a list holds entries of different units; only a signal needs one
        ok(&mono("pair = [f, 0.5]\nosc(saw, pair[0]) * pair[1]"));
        // `channels` makes a list of a signal's channels
        let p = ok(
            "config { channels: stereo }\ninstr a(f: hz) { osc(saw, f).pan.channels.map(fn(c) { c * 0.5 }) }",
        );
        assert_eq!(p.instruments[0].graph.channels(), 2);
        ok(
            "config { channels: stereo }\ninstr a(f: hz) { [l, r] = [osc(saw, f), osc(sine, f)]\n[r, l] }",
        );

        ok(&mono("osc(saw, f).pan.map(fn(c) { c }).sum"));
        ok(&mono("osc(saw, f)[0]"));
        assert_eq!(
            msg(&mono(
                "[[osc(saw, f), osc(saw, f)], [osc(saw, f), osc(saw, f)]]"
            )),
            "an array of arrays can't be used as a signal"
        );
        assert_eq!(
            msg(&mono("[l, r, c] = [osc(saw, f), osc(sine, f)]\nl")),
            "this array has 2 entries, but 3 name(s) are given"
        );
    }

    #[test]
    fn layouts_convert_automatically_where_the_spec_says_they_do() {
        // mono into 5.1 is centred; stereo into 5.1 takes the front pair
        ok("config { channels: surround(5.1) }\ninstr a(f: hz) { osc(saw, f) }");
        ok("config { channels: surround(5.1) }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f)] }");
        // 5.1 bus read by a stereo master track folds down
        ok(
            "config { channels: stereo }\nbus hall: surround(5.1)\ninstr a(f: hz) { out = osc(saw, f)\nhall += out }\ntrack t { out = hall }",
        );
        // unnamed channel lists are taken as whatever layout has that many
        ok(
            "config { channels: quad }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f), osc(saw, f), osc(saw, f)] }",
        );

        let d = first_error(
            "config { channels: stereo }\ninstr a(f: hz) { [osc(saw, f), osc(saw, f), osc(saw, f)] }",
        );
        assert_eq!(d.message, "can't convert ch[3] to stereo automatically");
        let d = first_error(
            "config { channels: surround(5.1) }\nbus q: quad\ninstr a(f: hz) { q += [osc(saw, f), osc(saw, f), osc(saw, f), osc(saw, f)].to_layout(surround(7.1)) }",
        );
        assert_eq!(
            d.message,
            "can't convert ch[4] to surround(7.1) automatically"
        );
    }

    #[test]
    fn to_layout_converts_explicitly() {
        ok(
            "config { channels: surround(5.1) }\nbus s: stereo\ninstr a(f: hz) { out = osc(saw, f).pan\ns += out.to_layout(stereo) }",
        );
        assert_eq!(
            msg("instr a(f: hz) { osc(saw, f).to_layout(wobble) }"),
            "unknown channel layout `wobble`"
        );
        ok("instr a(f: hz) { osc(saw, f).to_layout(mono) }");
    }

    #[test]
    fn buses_and_tracks_carry_their_layouts() {
        let p = ok("
config { channels: stereo }
bus space: stereo
bus lfe: mono
instr a(f: hz) { out = osc(saw, f).pan(azimuth: -20deg)\nspace += out\nlfe += out }
track room { out = space.reverb(room: 0.8) }
track sub { out = lfe.lpf(cutoff: 100hz) }");
        assert_eq!(p.bus_layouts, [Layout::Stereo, Layout::Mono]);
        assert_eq!(
            p.tracks[1].def.inputs.len(),
            2,
            "a stereo bus is two inputs"
        );
        assert_eq!(p.tracks[2].def.inputs.len(), 1);
        // the sub track works in mono but delivers stereo to the master
        assert_eq!(p.tracks[2].def.chain.as_ref().unwrap().channels(), 2);
        // routing to a bus delivers in that bus's layout
        let routed = ok("
config { channels: stereo }
bus mono_bus: mono
track t { instrument = i\nout = it.to(mono_bus) }
track u { out = mono_bus }
instr i(f: hz) { osc(saw, f).pan }");
        assert_eq!(routed.tracks[1].def.chain.as_ref().unwrap().channels(), 1);
    }


    // ---- the performance layer ------------------------------------------------


    /// (start in cycles, value) of every event beginning in the first `cycles` cycles.



    #[test]
    fn osc_out_sends_a_tracks_signal() {
        let track = |body: &str| {
            format!(
                "instr pluck(freq: hz) {{ osc(saw, freq) }}\ntrack lead {{\n instrument = pluck\n {body}\n}}\n"
            )
        };
        let p = ok(&track("out = it.osc_out(\"/vj/level\", port: 9000)"));
        assert_eq!(p.taps.len(), 1);
        assert_eq!(
            (
                p.taps[0].address.as_str(),
                p.taps[0].host.as_str(),
                p.taps[0].port,
                p.taps[0].rate
            ),
            ("/vj/level", "127.0.0.1", 9000, 30.0)
        );
        let p = ok(&track(
            "out = it.osc_out(\"/a\", host: \"10.0.0.2\", port: 9001, rate: 5hz).osc_out(\"/b\", port: 9001)",
        ));
        assert_eq!(
            p.taps
                .iter()
                .map(|t| (t.address.as_str(), t.host.as_str(), t.rate))
                .collect::<Vec<_>>(),
            [("/a", "10.0.0.2", 5.0), ("/b", "127.0.0.1", 30.0)]
        );
        assert_eq!(
            msg(&track("out = it.osc_out(\"vj\", port: 9000)")),
            "an OSC address starts with `/`"
        );
        assert_eq!(
            msg(&track("out = it.osc_out(\"/a\")")),
            "`osc_out` needs a `port:`"
        );
        assert_eq!(
            msg(&track("out = it.osc_out(port: 9000)")),
            "`osc_out` needs an address"
        );
        assert_eq!(
            msg(&track("out = it.osc_out(\"/a\", port: 0)")),
            "a port is a whole number from 1 to 65535"
        );
        assert_eq!(
            msg(&track("out = it.osc_out(\"/a\", port: 9000, rate: 900hz)")),
            "`rate` is from 0.5hz to 200hz"
        );
        assert_eq!(
            msg(&track("out = it.osc_out(\"/a\", prot: 9000)")),
            "`osc_out` has no argument named `prot`"
        );
        assert_eq!(
            msg(&track("out = it.osc_out(5, port: 9000)")),
            "an OSC address is text in quotes"
        );
        ok("instr a(f: hz) { osc(saw, f).osc_out(\"/a\", port: 9000) }");
        assert_eq!(
            msg("fn a(x) { x.osc_out(\"/a\", port: 9000) }"),
            "`osc_out` can't be used inside a function"
        );
    }


    #[test]
    fn mixer_lines_are_checked() {
        let tracks = |body: &str| {
            format!(
                "instr pluck(freq: hz) {{ osc(saw, freq) }}\ntrack lead {{\n instrument = pluck\n {body}\n}}\n"
            )
        };
        ok(&tracks("gain = -3db\n pan = 20deg"));
        ok(&tracks("out = it.lpf(cutoff: 2khz)\n gain = -3db"));
        assert_eq!(
            msg(&tracks("gain = -3db\n gain = -1db")),
            "a track has only one `gain` line"
        );
        assert!(msg(&tracks("gain = 20deg")).contains("expects"));
        assert!(msg(&tracks("pan = 20")).contains("azimuth"));
        // there is nothing to mix without a sound
        assert_eq!(
            msg("track fx {\n gain = -3db\n}"),
            "track `fx` never sets `out`"
        );
    }


    /// The notes of a clip with the controls each carries, in time order.





















    #[test]
    fn reports_errors_from_several_instruments_without_cascading() {
        let errs = errors(
            "
instr a() { osc(saw, 440) }
instr b() { osc(saw, 440) }
",
        );
        assert_eq!(
            errs.len(),
            2,
            "one per broken instrument, none for the note: {errs:?}"
        );
    }

    #[test]
    fn a_track_parameter_is_a_control_the_track_can_read() {
        let src = "instr p(freq: hz) { osc(saw, freq) }\ntrack hall(tone: hz = 2khz, drive: 0..1 = 0.2) {\n  instrument = p\n  out = it.lpf(cutoff: tone).gain(drive)\n}\n";
        let p = ok(src);
        let names: Vec<&str> = p.controls.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["hall::tone", "hall::drive"]);
        assert_eq!(p.controls[0].initial, 2000.0);
        assert_eq!(
            msg("track t(tone: hz) { out = it }"),
            "track parameter `tone` needs a starting value"
        );
        assert_eq!(
            help("track t(tone: hz) { out = it }").as_deref(),
            Some("for example `track bass(tone: hz = 2khz)`")
        );
    }


}
