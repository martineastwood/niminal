//! Compiling nodes and instruments, including kits and instrument resolution.

use super::*;

/// A node's chain, plus the instrument named by its `instrument = ...` line.
/// `None` when that instrument failed to compile, which is already reported.
pub(super) fn compile_node(
    item: &Item,
    instruments: &[Instrument],
    failed: &[&str],
    sample_names: &[String],
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
) -> Res<Option<NodeInfo>> {
    let (decl, collection) = match item {
        Item::Node(decl) => (decl, true),
        Item::Fx(decl) => (decl, false),
        _ => unreachable!("only nodes and fx declarations have playback chains"),
    };
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
                        format!("a node has only one `{}` line", name.name),
                        name.span,
                    ));
                }
                *line = Some(value);
            }
            Stmt::Bind { name, value } if name.name == "instrument" => {
                if instrument_line.is_some() {
                    return Err(Diagnostic::new("a node has only one instrument", name.span));
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
        &decl.name, &rest, collection, &mixer, tempo, registry, names,
    )?;
    let voices: Vec<&Instrument> = instrument
        .map(|i| {
            if instruments[i].kit_voices.is_empty() {
                vec![&instruments[i]]
            } else {
                instruments[i]
                    .kit_voices
                    .iter()
                    .map(|v| &instruments[v.instrument])
                    .collect()
            }
        })
        .unwrap_or_default();
    let mut voice_sends: Vec<_> = voices.iter().flat_map(|i| i.graph.send_buses()).collect();
    voice_sends.sort_unstable();
    voice_sends.dedup();
    let mut voice_buffer_writes: Vec<_> = voices
        .iter()
        .flat_map(|i| i.graph.buffer_writes().iter().copied())
        .collect();
    voice_buffer_writes.sort_unstable();
    voice_buffer_writes.dedup();
    let mut voice_buffer_reads: Vec<_> = voices
        .iter()
        .flat_map(|i| i.graph.buffer_reads().iter().copied())
        .collect();
    voice_buffer_reads.sort_unstable();
    voice_buffer_reads.dedup();
    Ok(Some(NodeInfo {
        accepts_notes: collection,
        taps: chain.taps.clone(),
        name: decl.name.name.clone(),
        instrument,
        defaults,
        def: NodeDef {
            meter: None,
            transition: (0, 0.0),
            reset_state: false,
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

/// Assemble a named kit without combining the instruments' DSP graphs.
pub(super) fn compile_instrument_kit(
    decl: &SampleDecl,
    entries: &[(Ident, Expr)],
    instruments: &[Instrument],
    failed: &[&str],
    state: &mut SampleState,
    tempo: Tempo,
) -> Res<Instrument> {
    if entries.is_empty() {
        return Err(Diagnostic::new(
            "a kit needs at least one instrument",
            decl.source_span,
        ));
    }
    let mut voices = Vec::new();
    let mut members = Vec::new();
    for (name, value) in entries {
        if members.contains(&name.name) {
            return Err(Diagnostic::new(
                format!("kit member `{}` is defined twice", name.name),
                name.span,
            ));
        }
        let Some((instrument, defaults)) =
            resolve_instrument(value, instruments, failed, &state.names, tempo)?
        else {
            return Err(Diagnostic::new(
                "kit member instrument failed to compile",
                value.span,
            ));
        };
        if !instruments[instrument].kit_voices.is_empty() {
            return Err(Diagnostic::new(
                "use a sample or synth instrument as a kit member",
                value.span,
            ));
        }
        if !instruments[instrument].members.is_empty() && !defaults.contains_key("sample") {
            return Err(Diagnostic::new(
                "choose a member of the sample kit, such as `drums.snare`",
                value.span,
            ));
        }
        members.push(name.name.clone());
        if !state.names.contains(&name.name) {
            state.names.push(name.name.clone());
        }
        voices.push(crate::program::KitVoice {
            instrument,
            defaults,
        });
    }
    let mut graph = niminal_engine::GraphBuilder::new();
    graph.param("sample", 0.0).expect("a fresh graph");
    graph.param("gain", 1.0).expect("a fresh graph");
    let silence =
        vec![niminal_engine::Src::Const(0.0); instruments[voices[0].instrument].graph.channels()];
    let mut kit = Instrument::new(
        decl.name.name.clone(),
        Arc::new(graph.build_channels(&silence)),
        vec![
            crate::program::Param {
                name: "sample".into(),
                unit: crate::unit::Unit::Num,
                range: None,
                default: Some(0.0),
            },
            crate::program::Param {
                name: "gain".into(),
                unit: crate::unit::Unit::Db,
                range: None,
                default: Some(1.0),
            },
        ],
    );
    kit.members = members;
    kit.kit_voices = voices;
    Ok(kit)
}

/// `pluck`, `pluck(bright: 0.6)`, `drums.snare`, or `drums.snare(pitch: -2st)`.
/// `None` when the instrument failed earlier.
pub(super) fn resolve_instrument(
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
    if !instr.kit_voices.is_empty()
        && let Some(member) = member
    {
        let local = instr
            .members
            .iter()
            .position(|m| m == member)
            .ok_or_else(|| {
                Diagnostic::new(
                    format!("there is no member `{member}` in the kit `{name}`"),
                    span,
                )
            })?;
        let voice = &instr.kit_voices[local];
        let mut defaults = voice.defaults.clone();
        defaults.extend(literal_args(
            args,
            &instruments[voice.instrument],
            sample_names,
            tempo,
        )?);
        return Ok(Some((voice.instrument, defaults)));
    }
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
