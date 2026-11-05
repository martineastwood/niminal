//! Samples, SFZ banks and buffers declared in a program.

use super::*;

/// `mono bass(...) { legato = true  glide = 80ms  priority = lowest }` and
/// `instr pad(...) { max = 8  steal = quietest  same_note = release  choke = group }`.
pub(super) fn voice_options(
    def: &InstrDef,
    instrument: &mut Instrument,
    state: &mut SampleState,
) -> Res<()> {
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
                "a setting is a name and a value, as in `max = 8`",
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
                            .with_help("for example `choke = bass`"),
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
                    return Err(
                        Diagnostic::new("`pitch` is the name of a parameter", value.span)
                            .with_help(
                                "for example `pitch = tone`, for `instr bass(tone: hz = 55hz)`",
                            ),
                    );
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
pub(super) struct SampleState {
    /// The names of the samples in kits, which a pattern may use.
    pub(super) names: Vec<String>,
    /// The names of choke groups.
    pub(super) groups: Vec<String>,
    /// Lone samples already loaded, for `.slices` to cut up and `.data` to expose.
    pub(super) loaded: HashMap<String, LoadedSample>,
}

pub(super) struct LoadedSample {
    pub(super) channels: Vec<Arc<niminal_engine::Table>>,
    pub(super) duration: Option<Expr>,
    pub(super) repitch: bool,
}

/// The kit made by cutting `data` into `count` equal pieces called `s0`, `s1`...
pub(super) fn slice_members(
    decl: &SampleDecl,
    sample: &Ident,
    count: f64,
    at: &[f64],
    sensitivity: Option<f64>,
    state: &SampleState,
) -> Res<Vec<Member>> {
    let Some(loaded) = state.loaded.get(&sample.name) else {
        let mut d = Diagnostic::new(
            format!("no sample named `{}` to slice", sample.name),
            sample.span,
        );
        if let Some(c) = closest(&sample.name, state.loaded.keys().map(String::as_str)) {
            d = d.with_help(format!("did you mean `{c}`?"));
        } else {
            d = d.with_help("declare it first with `name = sample(\"file.wav\")`");
        }
        return Err(d);
    };
    let data = &loaded.channels;
    let frames = data.first().map_or(0, |c| c.len());
    if count.fract() != 0.0 || count < 1.0 || count as usize > frames {
        return Err(Diagnostic::new(
            format!("a sample of {frames} frames can be cut into 1 to {frames} slices"),
            decl.source_span,
        ));
    }
    let mut count = count as usize;
    // where each piece starts: the hits found in the sample, equal shares of it, or the places the code gives as shares of its length
    let starts: Vec<usize> = if let Some(sensitivity) = sensitivity {
        let mixed: Vec<f32> = (0..frames)
            .map(|i| data.iter().map(|c| c.get(i)).sum::<f32>() / data.len() as f32)
            .collect();
        let rate = data
            .first()
            .map_or(48_000.0, |c| c.sample_rate_or(48_000.0));
        let found =
            niminal_engine::ops::detect_transients_at(&mixed, rate, count, sensitivity as f32);
        count = found.len();
        found
    } else if at.is_empty() {
        (0..count).map(|i| i * (frames / count)).collect()
    } else {
        if at.iter().any(|p| !(0.0..1.0).contains(p)) || at.windows(2).any(|w| w[1] <= w[0]) {
            return Err(Diagnostic::new(
                "slice places are shares of the sample from 0 up to (not including) 1, each later than the one before",
                decl.source_span,
            ));
        }
        at.iter()
            .map(|p| (p * frames as f64).round() as usize)
            .collect()
    };
    if starts.windows(2).any(|w| w[1] <= w[0]) || starts.last().is_some_and(|&s| s >= frames) {
        return Err(Diagnostic::new(
            format!("the slice places are too close together for a sample of {frames} frames"),
            decl.source_span,
        ));
    }
    Ok((0..count)
        .map(|i| Member {
            name: format!("s{i}"),
            channels: data.clone(),
            slice: Some((starts[i], starts.get(i + 1).copied().unwrap_or(frames))),
            duration: loaded.duration.clone(),
            repitch: loaded.repitch,
            loop_mode: niminal_engine::ops::LoopMode::None,
            interp: niminal_engine::ops::SampleInterp::Cubic,
        })
        .collect())
}

/// A `sample` or `kit` declaration as an instrument.
pub(super) fn compile_sample(
    decl: &SampleDecl,
    samples: &Samples,
    tempo: Tempo,
    registry: &Registry,
    names: &mut Names,
    state: &mut SampleState,
) -> Res<Instrument> {
    if let SampleSource::Sfz(path) = &decl.source {
        return compile_sfz(decl, path, samples, tempo, registry, names);
    }
    let mut root = DEFAULT_ROOT_HZ;
    let mut chokes: Vec<&Arg> = Vec::new();
    let mut duration: Option<Expr> = None;
    let mut repitch = false;
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
                    duration = Some(Expr {
                        kind: ExprKind::Num {
                            value: *value * 60.0 / tempo.bpm,
                            unit: Some("sec".into()),
                        },
                        span: arg.value.span,
                    })
                }
                _ => {
                    return Err(Diagnostic::new(
                        "`beats` is how many beats the loop lasts, such as `4`",
                        arg.value.span,
                    ));
                }
            },
            "duration" => duration = Some(arg.value.clone()),
            "mode" => {
                repitch = match &arg.value.kind {
                    ExprKind::Name(n) if n == "stretch" => false,
                    ExprKind::Name(n) if n == "repitch" => true,
                    _ => {
                        return Err(Diagnostic::new(
                            "sample mode is `stretch` or `repitch`",
                            arg.value.span,
                        ));
                    }
                };
            }
            "loop" | "interp" => {} // validated below
            other => {
                let mut d =
                    Diagnostic::new(format!("`{other}` isn't an option of a sample"), key.span);
                d = match closest(
                    other,
                    ["root", "choke", "beats", "duration", "loop", "interp", "mode"],
                ) {
                    Some(c) => d.with_help(format!("did you mean `{c}`?")),
                    None => d.with_help(
                        "the options are `root`, `choke`, `beats`, `duration`, `loop`, `interp` and `mode`",
                    ),
                };
                return Err(d);
            }
        }
    }
    if duration.is_some() && (!matches!(decl.source, SampleSource::Path(_)) || decl.is_kit) {
        return Err(Diagnostic::new(
            "only a single sample can be fitted to a length",
            decl.source_span,
        ));
    }
    let file_error = |e: String| Diagnostic::new(e, decl.source_span);
    let mut members: Vec<Member> = match (&decl.source, decl.is_kit) {
        (SampleSource::Path(path), true) => samples
            .load_kit(path)
            .map_err(file_error)?
            .into_iter()
            .map(|(name, channels)| Member {
                name,
                channels,
                slice: None,
                duration: None,
                repitch,
                loop_mode: niminal_engine::ops::LoopMode::None,
                interp: niminal_engine::ops::SampleInterp::Cubic,
            })
            .collect(),
        (SampleSource::Path(path), false) => {
            let channels = samples.load(path).map_err(file_error)?;
            state.loaded.insert(
                decl.name.name.clone(),
                LoadedSample {
                    channels: channels.clone(),
                    duration: duration.clone(),
                    repitch,
                },
            );
            vec![Member {
                name: decl.name.name.clone(),
                channels,
                slice: None,
                duration: duration.clone(),
                repitch,
                loop_mode: loop_mode_of(&decl.options)?,
                interp: niminal_engine::ops::SampleInterp::Cubic,
            }]
        }
        (
            SampleSource::Slices {
                sample,
                count,
                at,
                sensitivity,
            },
            _,
        ) => {
            let mut members = slice_members(decl, sample, *count, at, *sensitivity, state)?;
            let mode = loop_mode_of(&decl.options)?;
            for m in &mut members {
                m.loop_mode = mode;
                if decl
                    .options
                    .iter()
                    .any(|a| a.name.as_ref().is_some_and(|n| n.name == "mode"))
                {
                    m.repitch = repitch;
                }
            }
            members
        }
        (SampleSource::Table(expr), false) => {
            let value = compile_const(expr, tempo, registry, names)?;
            let table = table_from_const(value, decl.source_span)?;
            let channels = vec![table];
            state.loaded.insert(
                decl.name.name.clone(),
                LoadedSample {
                    channels: channels.clone(),
                    duration: None,
                    repitch,
                },
            );
            vec![Member {
                name: decl.name.name.clone(),
                channels,
                slice: None,
                duration: None,
                repitch,
                loop_mode: loop_mode_of(&decl.options)?,
                interp: niminal_engine::ops::SampleInterp::Cubic,
            }]
        }
        (SampleSource::Instruments(_), _) => {
            unreachable!("instrument kits compile after instruments")
        }
        (SampleSource::Sfz(_), _) => unreachable!("an SFZ file compiles on its own"),
        (SampleSource::Table(_), true) => {
            return Err(Diagnostic::new(
                "a kit is a folder of files or `.slices`, not a table",
                decl.source_span,
            ));
        }
    };
    if let Some(interp) = interp_of(&decl.options)? {
        for member in &mut members {
            member.interp = interp;
        }
    }
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
        SamplerSource::Members {
            members: &members,
            is_kit: decl.is_kit,
            root_hz: root,
        },
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
                        .with_help("for example `.choke(hats)`"),
                );
            }
            (_, true) => {
                return Err(
                    Diagnostic::new("a kit names the samples in each group", value.span)
                        .with_help("for example `.choke(hats(hat_open, hat_closed))`"),
                );
            }
        }
    }
    names.samplers.insert(
        decl.name.name.clone(),
        SamplerDef {
            members,
            root_hz: root,
            bank: None,
            instrument: instrument.clone(),
        },
    );
    Ok(instrument)
}

/// `.interp(sinc)` among a sample's options.
pub(super) fn interp_of(options: &[Arg]) -> Res<Option<niminal_engine::ops::SampleInterp>> {
    let Some(arg) = options
        .iter()
        .find(|a| a.name.as_ref().is_some_and(|n| n.name == "interp"))
    else {
        return Ok(None);
    };
    match &arg.value.kind {
        ExprKind::Name(n) if n == "linear" => Ok(Some(niminal_engine::ops::SampleInterp::Linear)),
        ExprKind::Name(n) if n == "cubic" => Ok(Some(niminal_engine::ops::SampleInterp::Cubic)),
        ExprKind::Name(n) if n == "sinc" => Ok(Some(niminal_engine::ops::SampleInterp::Sinc)),
        _ => Err(Diagnostic::new(
            "`interp` is `linear`, `cubic` or `sinc`",
            arg.value.span,
        )),
    }
}

/// An `sfz("file.sfz")` declaration as an instrument that plays the file's regions by key and velocity.
pub(super) fn compile_sfz(
    decl: &SampleDecl,
    path: &str,
    samples: &Samples,
    tempo: Tempo,
    registry: &Registry,
    names: &mut Names,
) -> Res<Instrument> {
    use niminal_engine::ops::{Region, RoundRobin};
    for arg in &decl.options {
        match arg.name.as_ref().map(|n| n.name.as_str()) {
            Some("interp") => {}
            Some(other) => {
                return Err(Diagnostic::new(
                    format!("`{other}` isn't an option of an SFZ file"),
                    arg.name.as_ref().map_or(arg.value.span, |n| n.span),
                )
                .with_help("the SFZ file sets everything but `interp`"));
            }
            None => {
                return Err(Diagnostic::new(
                    "options are written `name: value`",
                    arg.value.span,
                ));
            }
        }
    }
    let interp = interp_of(&decl.options)?.unwrap_or_default();
    let loaded = samples
        .load_sfz(path)
        .map_err(|e| Diagnostic::new(e, decl.source_span))?;
    // regions that cover the same keys and velocities take turns from one shared count
    let mut counters: HashMap<(u8, u8, u8, u8, u32), Arc<RoundRobin>> = HashMap::new();
    let mut regions = Vec::with_capacity(loaded.len());
    // each choke group the file names gets a counter that all the instrument's voices share
    let mut group_index: HashMap<i64, usize> = HashMap::new();
    for sample in &loaded {
        for group in [sample.region.group, sample.region.off_by]
            .into_iter()
            .flatten()
        {
            let next = group_index.len();
            group_index.entry(group).or_insert(next);
        }
    }
    for sample in loaded {
        let (r, info) = (&sample.region, sample.info);
        let frames = sample.channels.first().map_or(0, |c| c.len());
        let end = r.end.map_or(frames, |e| (e + 1).min(frames));
        if r.offset >= end {
            continue;
        }
        let range = (r.offset > 0 || end < frames).then_some((r.offset, end));
        let file_loop = info.loop_frames.map(|(from, to)| (from, to + 1));
        let kind = r.loop_kind.unwrap_or(if file_loop.is_some() {
            crate::sfz::LoopKind::Continuous
        } else {
            crate::sfz::LoopKind::NoLoop
        });
        let loop_frames = (kind == crate::sfz::LoopKind::Continuous).then(|| {
            let from = r.loop_start.or(file_loop.map(|l| l.0)).unwrap_or(r.offset);
            let to = r
                .loop_end
                .map(|e| e + 1)
                .or(file_loop.map(|l| l.1))
                .unwrap_or(end);
            (from, to.min(end))
        });
        let key = r.keycenter.or(info.root_key).unwrap_or(60);
        let root_hz = (440.0 * ((f64::from(key) - 69.0) / 12.0).exp2()) as f32;
        let seq = r.seq.map(|(length, position)| {
            let counter = counters
                .entry((r.keys.0, r.keys.1, r.vels.0, r.vels.1, length))
                .or_default()
                .clone();
            (length, position, counter)
        });
        regions.push(crate::lower::SfzRegionDef {
            region: Region {
                keys: r.keys,
                vels: r.vels,
                rand: r.rand,
                seq,
                tune: r.tune,
                keytrack: r.keytrack,
                gain: 10.0_f32.powf(r.volume_db / 20.0),
                env: r.env,
                one_shot: kind == crate::sfz::LoopKind::OneShot,
                group: r.group.map(|g| group_index[&g]),
                off_by: r.off_by.map(|g| group_index[&g]),
                // the player for each channel is made where the instrument is built
                sampler: niminal_engine::ops::Sampler::new(
                    sample.channels[0].clone(),
                    root_hz,
                    None,
                ),
            },
            channels: sample.channels,
            root_hz,
            range,
            loop_frames: loop_frames.filter(|(from, to)| from < to),
            interp,
        });
    }
    if regions.is_empty() {
        return Err(Diagnostic::new(
            format!("{path} has no region with audio to play"),
            decl.source_span,
        ));
    }
    let bank = Arc::new(crate::lower::SfzBank {
        regions,
        groups: (0..group_index.len())
            .map(|_| RoundRobin::default())
            .collect(),
    });
    let instrument = compile_sampler(
        &decl.name,
        SamplerSource::Regions(&bank),
        tempo,
        registry,
        names,
    )?;
    names.samplers.insert(
        decl.name.name.clone(),
        SamplerDef {
            members: Vec::new(),
            root_hz: DEFAULT_ROOT_HZ,
            bank: Some(bank),
            instrument: instrument.clone(),
        },
    );
    Ok(instrument)
}

/// The pitch (c4) at which a sample plays as recorded unless it says otherwise.
pub(crate) const DEFAULT_ROOT_HZ: f32 = 261.625_56;

pub(super) fn loop_mode_of(options: &[Arg]) -> Res<niminal_engine::ops::LoopMode> {
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

pub(super) fn table_from_const(value: Datum, span: Span) -> Res<Arc<niminal_engine::Table>> {
    match value {
        Datum::Table(t) => Ok(t),
        Datum::Array(items) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items.iter() {
                match item {
                    Datum::Sig(s) if s.konst().is_some() && s.unit() == crate::unit::Unit::Num => {
                        values.push(s.konst().expect("the guard checked it is constant"));
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

pub(super) fn compile_buffer(
    decl: &BufferDecl,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
    options: &CompileOptions,
) -> Res<(String, Arc<niminal_engine::Buffer>)> {
    if let ExprKind::Call { name, args } = &decl.value.kind
        && name.name == "zeros"
    {
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
            if value.unit() != crate::unit::Unit::Num || !n.is_finite() || n < 1. || n.fract() != 0.
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

pub(super) fn allocate_buffer(
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

pub(super) fn owned_samples(options: &CompileOptions, name: &str) -> Samples {
    name.split_once("::")
        .and_then(|(owner, _)| options.namespace_samples.get(owner))
        .cloned()
        .unwrap_or_else(|| options.samples.clone())
}

pub(super) fn scoped_group(context: &str, name: &str) -> String {
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
