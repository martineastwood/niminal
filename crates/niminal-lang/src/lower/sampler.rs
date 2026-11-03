//! Sampler instruments: sample banks, SFZ regions and the sampler signal.

use super::*;

/// What the compiler works out about a loaded sample: how much energy it has, and where its hits are.
pub(super) struct SampleAnalysis {
    /// Held so the table stays alive, which keeps its address from being reused for different audio.
    _table: Arc<Table>,
    energy: f64,
    transients: Arc<[usize]>,
}

/// The analysis of a sample, made once however many times the piece is compiled. Every evaluation compiles
/// the whole piece again, and for a long loop these two passes over its audio were most of the time.
pub(super) fn sample_analysis(table: &Arc<Table>) -> Arc<SampleAnalysis> {
    use std::sync::{Mutex, OnceLock};
    /// Samples remembered before the oldest are forgotten.
    const KEPT: usize = 64;
    static SEEN: OnceLock<Mutex<HashMap<usize, Arc<SampleAnalysis>>>> = OnceLock::new();
    let key = Arc::as_ptr(table) as usize;
    let seen = SEEN.get_or_init(Default::default);
    if let Some(found) = seen.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return found.clone();
    }
    let energy = table
        .data()
        .iter()
        .map(|x| f64::from(*x).powi(2))
        .sum::<f64>();
    let transients = niminal_engine::ops::detect_transients(
        table.data(),
        table.sample_rate_or(48_000.0),
        usize::MAX,
    )
    .into();
    let made = Arc::new(SampleAnalysis {
        _table: table.clone(),
        energy,
        transients,
    });
    let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
    if seen.len() >= KEPT {
        seen.clear();
    }
    seen.insert(key, made.clone());
    made
}

/// One sample of a sampler instrument.
pub(crate) struct Member {
    pub name: String,
    /// One table per channel.
    pub channels: Vec<Arc<Table>>,
    /// The frames of the tables it plays, if not all of them.
    pub slice: Option<(usize, usize)>,
    /// Target duration of the whole source, also inherited by its slices.
    pub duration: Option<Expr>,
    pub repitch: bool,
    pub loop_mode: LoopMode,
    pub interp: niminal_engine::ops::SampleInterp,
}

/// What a sampler instrument plays.
pub(crate) enum SamplerSource<'a> {
    /// A sample, or the members of a kit.
    Members {
        members: &'a [Member],
        is_kit: bool,
        root_hz: f32,
    },
    /// The regions of an SFZ file.
    Regions(&'a SfzBank),
}

/// An instrument that plays recorded audio, selecting a kit member (or the regions of a multisample)
/// once per note.
pub(crate) fn compile_sampler(
    name: &Ident,
    source: SamplerSource,
    tempo: Tempo,
    registry: &Registry,
    names: &Names,
) -> Res<Instrument> {
    let (members, is_kit, root_hz, bank) = match source {
        SamplerSource::Members {
            members,
            is_kit,
            root_hz,
        } => (members, is_kit, root_hz, None),
        SamplerSource::Regions(bank) => {
            (&[][..], false, crate::compile::DEFAULT_ROOT_HZ, Some(bank))
        }
    };
    let mut lower = Lower::new(registry, names, tempo);
    lower.mode = Mode::Instrument;
    let mut params: Vec<Param> = Vec::new();
    let mut declare = |lower: &mut Lower, pname: &str, unit: Unit, default: f32| {
        params.push(Param {
            name: pname.to_string(),
            unit,
            range: None,
            default: Some(default),
        });
        lower
            .g
            .param(pname, default)
            .expect("sampler parameter names are distinct")
    };
    let freq = declare(&mut lower, "freq", Unit::Hz, root_hz);
    let pitch = declare(&mut lower, "pitch", Unit::Semitones, 0.0);
    let rate = declare(&mut lower, "rate", Unit::Num, 1.0);
    let stretch = declare(&mut lower, "stretch", Unit::Num, 1.0);
    let stretch_mode = declare(&mut lower, "stretch_mode", Unit::Num, 0.0);
    let loop_start = declare(&mut lower, "loop_start", Unit::Num, 0.0);
    let loop_end = declare(&mut lower, "loop_end", Unit::Num, 1.0);
    let loop_crossfade = declare(&mut lower, "loop_crossfade", Unit::Time, 0.0);
    let start = declare(&mut lower, "start", Unit::Time, 0.0);
    // where in the sample to start, as a share of its length (a note's `begin`, set by `chop`)
    let begin = declare(&mut lower, "begin", Unit::Num, 0.0);
    let pick = is_kit.then(|| declare(&mut lower, "sample", Unit::Num, 0.0));
    let gain = declare(&mut lower, GAIN_PARAM, Unit::Db, 1.0);

    let mut wires = vec![
        ("freq", freq),
        ("pitch", pitch),
        ("rate", rate),
        ("stretch", stretch),
        ("stretch_mode", stretch_mode),
        ("loop_start", loop_start),
        ("loop_end", loop_end),
        ("loop_crossfade", loop_crossfade),
        ("start", start),
        ("begin", begin),
    ];
    wires.extend(pick.map(|p| ("sample", p)));
    let result = match bank {
        None => lower.sampler_signal(name, members, root_hz, &wires)?,
        Some(bank) => {
            // the layers a multisample plays are chosen by how hard the note is played
            let vel = declare(&mut lower, "vel", Unit::Num, 1.0);
            wires.push(("vel", vel));
            lower.sfz_signal(name, bank, &wires)?
        }
    };
    let result = lower.mul(&result, &Sig::signal(gain, Unit::Num), Unit::Num, name.span)?;
    let mut instrument = Instrument::new(
        name.name.clone(),
        Arc::new(lower.g.build_channels(&result.srcs)),
        params,
    );
    if is_kit {
        instrument.members = members.iter().map(|m| m.name.clone()).collect();
    }
    Ok(instrument)
}

/// One region of an SFZ multisample with its audio, ready to play.
pub(crate) struct SfzRegionDef {
    /// One table per channel.
    pub channels: Vec<Arc<Table>>,
    pub region: niminal_engine::ops::Region,
    pub root_hz: f32,
    /// The frames it plays, if not all of them.
    pub range: Option<(usize, usize)>,
    /// The frames it loops, if it holds a note by looping.
    pub loop_frames: Option<(usize, usize)>,
    pub interp: niminal_engine::ops::SampleInterp,
}

/// An SFZ file's regions and the choke counters they share.
pub(crate) struct SfzBank {
    pub regions: Vec<SfzRegionDef>,
    pub groups: Arc<[niminal_engine::ops::RoundRobin]>,
}

/// Loaded sampler data reused when a sampler is called inside an instrument.
pub(crate) struct SamplerDef {
    pub members: Vec<Member>,
    pub root_hz: f32,
    /// The regions of an SFZ file, which stand in for the members.
    pub bank: Option<Arc<SfzBank>>,
    pub instrument: Instrument,
}

impl Lower<'_> {
    pub(super) fn sampler_signal(
        &mut self,
        name: &Ident,
        members: &[Member],
        root_hz: f32,
        wires: &[(&str, Src)],
    ) -> Res<Sig> {
        let too_many = |n: usize| {
            Diagnostic::new(
                format!(
                    "a sample with {n} channels is more than the {MAX_SAMPLE_CHANNELS} niminal supports"
                ),
                name.span,
            )
        };
        let mut wires = wires.to_vec();
        if let Some(duration) = members.first().and_then(|m| m.duration.as_ref()) {
            let signal = self.expr(duration)?;
            if signal.unit != Unit::Time || signal.srcs.len() != 1 {
                return Err(Diagnostic::new(
                    "sample duration needs a time, such as 2sec",
                    duration.span,
                ));
            }
            if signal.konst.is_some_and(|v| !v.is_finite() || v <= 0.0) {
                return Err(Diagnostic::new(
                    "sample duration must be positive",
                    duration.span,
                ));
            }
            wires.push(("duration", signal.srcs[0]));
        }
        // Members with the same channel layout share one selectable player per channel. A note
        // chooses its member once; it does not run and sum every other member's silent graph.
        let mut layouts = std::collections::BTreeMap::<usize, Vec<usize>>::new();
        for (i, member) in members.iter().enumerate() {
            let channels = member.channels.len();
            if channels > MAX_SAMPLE_CHANNELS {
                return Err(too_many(channels));
            }
            layouts.entry(channels).or_default().push(i);
        }
        // One reference per member, shared by all its playback channels. Pick the
        // loudest channel rather than mixing: anti-phase stereo must not cancel.
        let references: Vec<_> = members
            .iter()
            .map(|member| {
                member
                    .channels
                    .iter()
                    .map(|channel| (sample_analysis(channel).energy, channel))
                    .max_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, channel)| channel.clone())
            })
            .collect();
        let transients: Vec<Arc<[usize]>> = references
            .iter()
            .map(|reference| {
                reference.as_ref().map_or_else(
                    || Arc::from([]),
                    |table| sample_analysis(table).transients.clone(),
                )
            })
            .collect();
        let mut mix: Option<Sig> = None;
        for (channels, indices) in layouts {
            let mut srcs = Vec::with_capacity(channels);
            for channel in 0..channels {
                let mut players = vec![None; members.len()];
                for &i in &indices {
                    let member = &members[i];
                    let mut player = Sampler::new(member.channels[channel].clone(), root_hz, None)
                        .with_stretch_reference(
                            references[i].as_ref().expect("sample has channels").clone(),
                        )
                        .with_transients(transients[i].clone())
                        .with_repitch(member.repitch)
                        .with_loop(member.loop_mode)
                        .with_interp(member.interp);
                    if let Some((start, end)) = member.slice {
                        player = player.slice(start, end);
                    }
                    players[i] = Some(player);
                }
                srcs.push(
                    self.g
                        .add(KitSampler::new(players), &wires)
                        .map_err(|e| Diagnostic::new(e.to_string(), name.span))?,
                );
            }
            let sig = self.convert(
                &Sig::channels(srcs, Layout::from_count(channels)),
                self.names.master,
                name.span,
            )?;
            mix = Some(match mix {
                None => sig,
                Some(so_far) => self.binary(BinOp::Add, so_far, sig, name.span)?,
            });
        }
        Ok(mix.expect("a sample or kit has at least one sample"))
    }

    /// The sound of an SFZ multisample: per channel layout, one player that chooses its regions
    /// when a note starts.
    pub(super) fn sfz_signal(
        &mut self,
        name: &Ident,
        bank: &SfzBank,
        wires: &[(&str, Src)],
    ) -> Res<Sig> {
        let mut layouts = std::collections::BTreeMap::<usize, Vec<&SfzRegionDef>>::new();
        for def in &bank.regions {
            let channels = def.channels.len();
            if channels > MAX_SAMPLE_CHANNELS {
                return Err(Diagnostic::new(
                    format!(
                        "a sample with {channels} channels is more than the {MAX_SAMPLE_CHANNELS} niminal supports"
                    ),
                    name.span,
                ));
            }
            layouts.entry(channels).or_default().push(def);
        }
        let mut mix: Option<Sig> = None;
        for (channels, defs) in layouts {
            let mut srcs = Vec::with_capacity(channels);
            for channel in 0..channels {
                let players = defs
                    .iter()
                    .map(|def| {
                        let mut sampler =
                            Sampler::new(def.channels[channel].clone(), def.root_hz, None)
                                .with_interp(def.interp);
                        if let Some((start, end)) = def.range {
                            sampler = sampler.slice(start, end);
                        }
                        if let Some((start, end)) = def.loop_frames {
                            sampler = sampler.with_loop_frames(LoopMode::Forward, start, end);
                        }
                        niminal_engine::ops::Region {
                            sampler,
                            ..def.region.clone()
                        }
                    })
                    .collect();
                srcs.push(
                    self.g
                        .add(
                            niminal_engine::ops::MultiSampler::new(players, bank.groups.clone()),
                            wires,
                        )
                        .map_err(|e| Diagnostic::new(e.to_string(), name.span))?,
                );
            }
            let sig = self.convert(
                &Sig::channels(srcs, Layout::from_count(channels)),
                self.names.master,
                name.span,
            )?;
            mix = Some(match mix {
                None => sig,
                Some(so_far) => self.binary(BinOp::Add, so_far, sig, name.span)?,
            });
        }
        Ok(mix.expect("an SFZ file has at least one region"))
    }

    pub(super) fn call_sampler(
        &mut self,
        sampler: &SamplerDef,
        name: &Ident,
        args: &[Arg],
    ) -> Res<Datum> {
        if self.mode != Mode::Instrument || self.in_fn {
            return Err(Diagnostic::new(
                "a sampler can only be played inside an instrument",
                name.span,
            ));
        }
        let mut values = HashMap::new();
        for arg in args {
            let Some(key) = &arg.name else {
                return Err(Diagnostic::new(
                    "name the sampler arguments, as in `freq: freq`",
                    arg.value.span,
                ));
            };
            let Some(param) = sampler
                .instrument
                .params
                .iter()
                .find(|p| p.name == key.name)
            else {
                return Err(Diagnostic::new(
                    format!("`{}` has no parameter `{}`", name.name, key.name),
                    key.span,
                ));
            };
            let value = if key.name == "sample"
                && let ExprKind::Name(member) = &arg.value.kind
                && !self.scope.contains_key(member)
                && let Some(index) = sampler.instrument.members.iter().position(|m| m == member)
            {
                Sig::constant(Unit::Num, index as f64)
            } else {
                self.expr(&arg.value)?
            };
            if value.count() != 1 {
                return Err(Diagnostic::new(
                    format!("`{}` expects one value", key.name),
                    arg.value.span,
                ));
            }
            if value.unit != param.unit && !(key.name == GAIN_PARAM && value.unit == Unit::Num) {
                return Err(self.unit_mismatch(&key.name, param.unit, value.unit, &arg.value));
            }
            if key.name == "sample"
                && let Some(Datum::Sig(pick)) = self.scope.get("sample")
                && value.srcs == pick.srcs
            {
                let kit = (
                    sampler.instrument.members.clone(),
                    sampler.instrument.chokes.clone(),
                );
                if self.kit.as_ref().is_some_and(|previous| previous != &kit) {
                    return Err(Diagnostic::new(
                        "a forwarded `sample` parameter must select from the same kit",
                        arg.value.span,
                    ));
                }
                self.kit = Some(kit);
            }
            if values.insert(key.name.as_str(), value).is_some() {
                return Err(Diagnostic::new(
                    format!("`{}` is given twice", key.name),
                    key.span,
                ));
            }
        }
        let mut wires = Vec::new();
        let mut gain = Sig::constant(Unit::Num, 1.0);
        for param in &sampler.instrument.params {
            let value = values.remove(param.name.as_str()).unwrap_or_else(|| {
                Sig::constant(param.unit, f64::from(param.default.unwrap_or(0.0)))
            });
            if param.name == GAIN_PARAM {
                gain = value;
            } else {
                wires.push((param.name.as_str(), value.srcs[0]));
            }
        }
        let signal = match &sampler.bank {
            None => self.sampler_signal(name, &sampler.members, sampler.root_hz, &wires)?,
            Some(bank) => self.sfz_signal(name, bank, &wires)?,
        };
        self.mul(&signal, &gain, Unit::Num, name.span)
            .map(Datum::Sig)
    }
}
