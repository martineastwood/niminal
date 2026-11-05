//! Commands, arrangements and checking clips against their nodes.

use super::*;

impl Perform<'_> {
    // ---- commands ----------------------------------------------------------------

    /// A clip by name; the implicit node 0 can't be named.
    pub(super) fn node(&self, id: &Ident) -> Res<usize> {
        match self.cx.nodes.iter().skip(1).position(|t| t.name == id.name) {
            Some(i) => Ok(i + 1),
            None => {
                let mut d = Diagnostic::new(format!("no clip named `{}`", id.name), id.span);
                if let Some(c) = closest(
                    &id.name,
                    self.cx.nodes.iter().skip(1).map(|t| t.name.as_str()),
                ) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                Err(d)
            }
        }
    }

    pub(super) fn at_time(&self, at: &Option<AtPos>) -> Res<Time> {
        let bpb = self.cx.tempo.beats_per_bar;
        match at {
            None => Ok(Time::Beats(0.0)),
            Some(AtPos::Time(e) | AtPos::After(e)) => time_of(e, self),
            Some(AtPos::Marker { section, .. }) => Err(Diagnostic::new(
                format!("`{}.start` is only known inside an arrangement with `sections`", section.name),
                section.span,
            )),
            Some(AtPos::Bar { bar, beat }) => {
                let number = |e: &Expr| self.plain(e).map(|r| r.to_f64());
                let bar_n = number(bar)?;
                let beat_n = beat.as_ref().map(number).transpose()?.unwrap_or(1.0);
                if bar_n < 1.0 || beat_n < 1.0 {
                    return Err(Diagnostic::new("bars and beats count from 1", bar.span));
                }
                Ok(Time::Beats((bar_n - 1.0) * bpb + (beat_n - 1.0)))
            }
        }
    }

    pub(super) fn command(&self, stmt: &CommandStmt) -> Res<Vec<Scheduled>> {
        let at = self.at_time(&stmt.at)?;
        self.command_at(stmt, at)
    }

    /// The command placed at `at`, whatever the statement says.
    fn command_at(&self, stmt: &CommandStmt, at: Time) -> Res<Vec<Scheduled>> {
        let quantize = stmt.quantize;
        let schedule = |action| Scheduled {
            at,
            action,
            quantize,
        };
        // an arrangement is started to run its lines from now, and stopped to drop the ones still to come
        match &stmt.command {
            Command::Start { name, .. } if self.arrangements.contains_key(&name.name) => {
                let arrangement = &self.arrangements[&name.name];
                if !arrangement.tempos.is_empty() || !arrangement.lanes.is_empty() {
                    return Err(Diagnostic::new(
                        format!(
                            "`{0}` sets its own tempo or moves a parameter over the whole piece, which only a render of it can follow",
                            name.name
                        ),
                        name.span,
                    )
                    .with_help(format!("render it with `niminal render file.nml {}`", name.name)));
                }
                let tempo = self.cx.tempo;
                let base = at.to_seconds(tempo);
                return Ok(self.arrangements[&name.name]
                    .actions
                    .iter()
                    .filter(|a| !matches!(a.action, Action::Hush) || a.at.to_seconds(tempo) > 0.0)
                    .map(|a| Scheduled {
                        at: Time::Seconds(base + a.at.to_seconds(tempo)),
                        action: a.action.clone(),
                        quantize,
                    })
                    .collect());
            }
            Command::Stop(name) if self.arrangements.contains_key(&name.name) => {
                return Ok(vec![schedule(Action::Cancel {
                    arrangement: name.name.clone(),
                })]);
            }
            _ => {}
        }

        Ok(match &stmt.command {
            Command::Update { node, what } => {
                if self.cx.failed.contains(&node.name.as_str()) {
                    return Ok(Vec::new());
                }
                let t = self.node(node)?;
                let clip = self.to_clip(self.eval(what, None)?, &node.name, what.span)?;
                let clip = self.on_node(clip, t, node, what.span)?;
                vec![schedule(Action::Update { node: t, clip })]
            }
            Command::Start { name, .. } => {
                if self.cx.failed.contains(&name.name.as_str()) {
                    return Ok(Vec::new());
                }
                let mut actions = Vec::new();
                for node in self.targets(name, "start")? {
                    actions.push(schedule(self.start(node, name)?));
                }
                actions
            }
            Command::Stop(t) => self
                .targets(t, "stop")?
                .into_iter()
                .map(|node| schedule(Action::Stop { node }))
                .collect(),
            Command::Mute(t) => self
                .targets(t, "mute")?
                .into_iter()
                .map(|node| schedule(Action::Mute { node }))
                .collect(),
            Command::Unmute(t) => self
                .targets(t, "unmute")?
                .into_iter()
                .map(|node| schedule(Action::Unmute { node }))
                .collect(),
            Command::Solo(t) => self
                .targets(t, "solo")?
                .into_iter()
                .map(|node| schedule(Action::Solo { node }))
                .collect(),
            Command::Unsolo(Some(t)) => self
                .targets(t, "unsolo")?
                .into_iter()
                .map(|node| schedule(Action::Unsolo { node: Some(node) }))
                .collect(),
            Command::Unsolo(None) => vec![schedule(Action::Unsolo { node: None })],
            Command::Hush => vec![schedule(Action::Hush)],
            Command::Panic => vec![schedule(Action::Panic)],
            Command::Reset | Command::New => vec![schedule(Action::Reset)],
            Command::Capture { length, name } => {
                let bars = self.cycles(length)?.to_f64();
                if bars <= 0.0 {
                    return Err(Diagnostic::new(
                        "capture needs a length above zero",
                        length.span,
                    ));
                }
                vec![schedule(Action::Capture {
                    bars,
                    name: name.name.clone(),
                })]
            }
            // changing what a part plays is for a live session; in a file it does nothing
            Command::Edit { .. } => Vec::new(),
        })
    }

    /// The clips a command on `name` is about: the clips of a scene, or the one clip.
    pub(super) fn targets(&self, name: &Ident, verb: &str) -> Res<Vec<usize>> {
        if let Some(PerfValue::Scene(scene)) = self.bindings.get(&name.name) {
            return Ok(scene.clips.clone());
        }
        if let Ok(node) = self.node(name) {
            return Ok(vec![node]);
        }
        if self.arrangements.contains_key(&name.name) {
            return Err(Diagnostic::new(
                format!(
                    "`{}` is an arrangement, which can only be started and stopped, not `{verb}`",
                    name.name
                ),
                name.span,
            ));
        }
        let mut d = match self.bindings.get(&name.name) {
            Some(PerfValue::Clip(_)) => Diagnostic::new(
                format!(
                    "clip `{0}` has no instrument, so `{0}.{verb}` has nothing to {verb}",
                    name.name
                ),
                name.span,
            )
            .with_help(format!(
                "give it one: `clip {} {{ instrument: ... }}`",
                name.name
            )),
            Some(PerfValue::Pattern(_)) => Diagnostic::new(
                format!(
                    "`{0}` is a pattern, not a clip, so `{0}.{verb}` has nothing to {verb}",
                    name.name
                ),
                name.span,
            )
            .with_help("a clip has an instrument and a `notes:` lane"),
            _ if self.cx.instruments.iter().any(|i| i.name == name.name) => Diagnostic::new(
                format!("`{}` is an instrument, not a clip", name.name),
                name.span,
            )
            .with_help(format!(
                "a clip plays it: `clip riff {{ instrument: {}, notes: [c4 e4] }}`",
                name.name
            )),
            _ => Diagnostic::new(format!("no scene or clip named `{}`", name.name), name.span),
        };
        if d.help.is_none() {
            let scenes = self
                .bindings
                .iter()
                .filter(|(_, v)| matches!(v, PerfValue::Scene(_)))
                .map(|(k, _)| k.as_str());
            let clips = self.cx.nodes.iter().skip(1).map(|t| t.name.as_str());
            if let Some(c) = closest(&name.name, scenes.chain(clips)) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
        }
        Ok(d).and_then(Err)
    }

    /// The action that starts the clip on `node`.
    pub(super) fn start(&self, node: usize, id: &Ident) -> Res<Action> {
        let name = self.cx.nodes[node].name.clone();
        let at = Ident {
            name: name.clone(),
            span: id.span,
        };
        let value = match self.bindings.get(&name) {
            Some(v) => v.clone(),
            None => return Err(Diagnostic::new(format!("no clip named `{name}`"), id.span)),
        };
        let clip = self.to_clip(value, &name, id.span)?;
        let clip = self.on_node(clip, node, &at, id.span)?;
        Ok(Action::Start { node, clip })
    }

    /// Scenes one after another, each for its length, then everything stops. A section may have its own
    /// tempo and meter, from the `tempo` and `meter` lists.
    pub(super) fn arrangement(&self, decl: &ArrangementDecl) -> Res<Arrangement> {
        for (name, list) in [("tempo", &decl.tempos), ("meter", &decl.meters)] {
            if let Some(list) = list
                && list.len() != decl.sections.len()
            {
                return Err(Diagnostic::new(
                    format!(
                        "`{name}` has {} where there are {} sections",
                        list.len(),
                        decl.sections.len()
                    ),
                    list.first().map_or(decl.name.span, |e| e.span),
                )
                .with_help(format!("give `{name}` one for each section")));
            }
        }
        let ramp = decl.tempo_env.as_ref().map(tempo_curve).transpose()?;
        let own = decl.tempos.is_some() || decl.meters.is_some() || ramp.is_some();
        let mut seconds = 0.0;
        let mut bars_so_far = 0.0;
        let mut actions = Vec::new();
        let mut previous: Vec<usize> = Vec::new();
        let mut tempos: Vec<(f64, Tempo)> = Vec::new();
        // each named section: its label, where it starts and ends in seconds, and its tempo for offsets
        let mut markers: Vec<(String, f64, f64, Tempo)> = Vec::new();
        for (i, section) in decl.sections.iter().enumerate() {
            let mut tempo = self.cx.tempo;
            if let Some(e) = decl.tempos.as_ref().map(|l| &l[i]) {
                tempo.bpm = crate::compile::bpm_of(e)?;
            }
            if let Some(e) = decl.meters.as_ref().map(|l| &l[i]) {
                (tempo.beats_per_bar, tempo.beat_unit) = crate::compile::meter_beats(e)?;
            }
            let length = self.cycles_in(&section.length, tempo.beats_per_bar)?;
            if length <= Rational::ZERO {
                return Err(Diagnostic::new(
                    "a section needs a length above zero",
                    section.length.span,
                ));
            }
            let at = Time::Seconds(seconds);
            let now = self.targets(&section.scene, "start")?;
            // a section stops what the one before it had going and this one does not, and starts its own
            for node in previous.iter().filter(|n| !now.contains(n)) {
                actions.push(Scheduled {
                    at,
                    action: Action::Stop { node: *node },
                    quantize: None,
                });
            }
            for node in &now {
                actions.push(Scheduled {
                    at,
                    action: self.start(*node, &section.scene)?,
                    quantize: None,
                });
            }
            previous = now;
            let bars = length.to_f64();
            let start_seconds = seconds;
            match &ramp {
                None => {
                    if own {
                        tempos.push((seconds, tempo));
                    }
                    seconds += bars * tempo.beats_per_bar * 60.0 / tempo.bpm;
                }
                Some(curve) => {
                    // the tempo changes gradually: a run of short steps, each at the tempo of its middle
                    let steps = (bars / RAMP_STEP_BARS).ceil().max(1.0) as usize;
                    for j in 0..steps {
                        let from = j as f64 * RAMP_STEP_BARS;
                        let width = (bars - from).min(RAMP_STEP_BARS);
                        let step = Tempo {
                            bpm: curve.bpm_at(bars_so_far + from + width / 2.0),
                            ..tempo
                        };
                        if tempos.last().is_none_or(|(_, t)| *t != step) {
                            tempos.push((seconds, step));
                        }
                        seconds += width * step.beats_per_bar * 60.0 / step.bpm;
                    }
                }
            }
            bars_so_far += bars;
            let label = match &section.label {
                Some(l) => Some(l),
                None if !decl.sections[..i]
                    .iter()
                    .any(|s| s.scene.name == section.scene.name || s.label.as_ref().is_some_and(|l| l.name == section.scene.name)) =>
                {
                    Some(&section.scene)
                }
                None => None,
            };
            if let Some(label) = label {
                if markers.iter().any(|m| m.0 == label.name) {
                    return Err(Diagnostic::new(
                        format!("two sections are called `{}`", label.name),
                        label.span,
                    )
                    .with_help("name one of them: `chorus.over(8bars) as last`"));
                }
                markers.push((label.name.clone(), start_seconds, seconds, tempo));
            }
        }
        if !decl.sections.is_empty() {
            actions.push(Scheduled {
                at: Time::Seconds(seconds),
                action: Action::Hush,
                quantize: None,
            });
        }
        // lines placed from the start of the arrangement, such as `at +4bars bass.mute`
        for stmt in &decl.commands {
            if let Some(AtPos::Marker {
                section,
                end,
                offset,
            }) = &stmt.at
            {
                let Some((_, start, finish, tempo)) =
                    markers.iter().find(|m| m.0 == section.name)
                else {
                    let mut d = Diagnostic::new(
                        format!("no section called `{}` in arrangement `{}`", section.name, decl.name.name),
                        section.span,
                    );
                    if let Some(c) = closest(&section.name, markers.iter().map(|m| m.0.as_str())) {
                        d = d.with_help(format!("did you mean `{c}`?"));
                    } else {
                        d = d.with_help("sections are named after their scene; a scene used twice needs `as name` on the repeats");
                    }
                    return Err(d);
                };
                let mut t = if *end { *finish } else { *start };
                if let Some((negative, e)) = offset {
                    // an offset counts at the tempo and meter of the section it is measured from
                    let off = time_of(e, self)?.to_seconds(*tempo);
                    t += if *negative { -off } else { off };
                }
                if t < 0.0 {
                    return Err(Diagnostic::new(
                        "that is before the start of the arrangement",
                        section.span,
                    ));
                }
                actions.extend(self.command_at(stmt, Time::Seconds(t))?);
            } else {
                actions.extend(self.command(stmt)?);
            }
        }
        let mut lanes = Vec::new();
        for lane in &decl.lanes {
            let (node, param) = (&lane.node, &lane.param);
            if !self.cx.nodes.iter().skip(1).any(|t| t.name == node.name) {
                let mut d = Diagnostic::new(format!("no clip named `{}`", node.name), node.span);
                if let Some(c) = closest(
                    &node.name,
                    self.cx.nodes.iter().skip(1).map(|t| t.name.as_str()),
                ) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                return Err(d);
            }
            let mut curve = env_lane(&lane.value)?;
            let who = format!(
                "arrangement `{}` moves `{}.{}`, but node `{}`",
                decl.name.name, node.name, param.name, node.name
            );
            self.bind_env(&mut curve, &node.name, &param.name, &who, param.span)?;
            lanes.push((format!("{}::{}", node.name, param.name), curve));
        }
        Ok(Arrangement {
            name: decl.name.name.clone(),
            actions,
            tempos,
            lanes,
        })
    }

    /// Check the clip against its playback node and instrument.
    pub(super) fn on_node(&self, mut clip: Clip, node: usize, id: &Ident, span: Span) -> Res<Clip> {
        self.bind_curves(&mut clip, node, id, span)?;
        self.check_clip(&clip, node, id, span)?;
        Ok(clip)
    }

    /// Envelope lanes move a parameter of the node: find it, and check the levels fit its unit.
    pub(super) fn bind_curves(&self, clip: &mut Clip, node: usize, id: &Ident, span: Span) -> Res<()> {
        let name = &self.cx.nodes[node].name;
        clip.fx_params = clip
            .lanes
            .iter()
            .filter_map(|(lane, _)| {
                lane.contains("::")
                    .then(|| self.cx.names.controls.iter().find(|c| c.name == *lane))
                    .flatten()
                    .map(|c| (lane.clone(), c.unit, c.smooth_seconds, f64::from(c.initial)))
            })
            .collect();
        for (lane, curve) in &mut clip.curves {
            let who = format!(
                "clip `{}` has an envelope lane `{}`, but node `{}`",
                clip.name,
                crate::compile::written(lane),
                id.name
            );
            self.bind_env(curve, name, lane, &who, span)?;
        }
        // a lane that is a signal and named after a parameter of the part moves it continuously
        clip.signals.clear();
        let probe = niminal_pattern::Span::new(Rational::ZERO, Rational::ZERO);
        for (lane, pattern) in &clip.lanes {
            let is_signal = pattern
                .query(probe)
                .first()
                .is_some_and(|h| h.whole.is_none());
            let control = if lane.contains("::") {
                lane.clone()
            } else {
                format!("{name}::{lane}")
            };
            if let Some(def) = self
                .cx
                .names
                .controls
                .iter()
                .find(|c| c.name == control)
                .filter(|_| is_signal)
            {
                clip.signals.push((lane.clone(), def.unit));
            }
        }
        Ok(())
    }

    /// Point `curve` at the part parameter `node::param` and check its levels fit that parameter's unit.
    /// `who` starts the message if there is no such parameter.
    pub(super) fn bind_env(
        &self,
        curve: &mut EnvLane,
        node: &str,
        param: &str,
        who: &str,
        span: Span,
    ) -> Res<()> {
        let control = if param.contains("::") {
            param.to_string()
        } else {
            format!("{node}::{param}")
        };
        let Some(def) = self.cx.names.controls.iter().find(|c| c.name == control) else {
            let params: Vec<&str> = self
                .cx
                .names
                .controls
                .iter()
                .filter_map(|c| c.name.strip_prefix(&format!("{node}::")))
                .collect();
            let mut d = Diagnostic::new(format!("{who} has no such parameter"), span).with_help(
                "name the fx parameter explicitly, as in `filter.cut: env[300hz 1bar 3khz]`",
            );
            if let Some(c) = closest(param, params) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
            return Err(d);
        };
        let tempo = Tempo::new(120.0);
        let levels =
            std::iter::once(&curve.start).chain(curve.segments.iter().map(|(_, _, level)| level));
        for level in levels {
            niminal_lang::engine_value(def.unit, *level, tempo).map_err(|e| {
                Diagnostic::new(
                    format!(
                        "envelope lane `{}` moves {}: {e}",
                        crate::compile::written(param),
                        def.name
                    ),
                    span,
                )
            })?;
        }
        curve.unit = Some(def.unit);
        Ok(())
    }

    /// Does `clip`'s notes and lanes make sense on `node`'s instrument?
    pub(super) fn check_clip(&self, clip: &Clip, node: usize, id: &Ident, span: Span) -> Res<()> {
        let info = &self.cx.nodes[node];
        let Some(instrument) = info.instrument.map(|i| &self.cx.instruments[i]) else {
            return Err(Diagnostic::new(
                format!("node `{}` has no instrument to play a clip on", id.name),
                id.span,
            )
            .with_help("add `instrument = ...` to the node"));
        };
        if clip.keys.is_some() {
            if clip.lane("notes").is_some() || !clip.scored.is_empty() {
                return Err(Diagnostic::new(
                    format!(
                        "clip `{}` has both `keys` and notes: it is played by one or the other",
                        clip.name
                    ),
                    span,
                ));
            }
            if !instrument
                .params
                .iter()
                .any(|p| p.name == instrument.pitch && p.unit == Unit::Hz)
            {
                return Err(Diagnostic::new(
                    format!(
                        "`{}` has no `{}` parameter for the keys to set",
                        instrument.name, instrument.pitch
                    ),
                    span,
                )
                .with_help(format!(
                    "give the instrument a `{}: hz` parameter",
                    instrument.pitch
                )));
            }
        }
        for (lane, _) in &clip.lanes {
            if lane == "notes" {
                let pitched = instrument
                    .params
                    .iter()
                    .any(|p| p.name == instrument.pitch && p.unit == Unit::Hz);
                if !pitched && instrument.members.is_empty() {
                    return Err(Diagnostic::new(
                        format!(
                            "`{}` has no `{}` parameter for the notes to set",
                            instrument.name, instrument.pitch
                        ),
                        span,
                    )
                    .with_help(format!(
                        "give the instrument a `{}: hz` parameter",
                        instrument.pitch
                    )));
                }
                continue;
            }
            if UNIVERSAL_LANES.contains(&lane.as_str())
                || instrument.params.iter().any(|p| p.name == *lane)
            {
                continue;
            }
            if lane == "to" {
                return Err(Diagnostic::new(
                    format!("the `{lane}` lane isn't supported yet"),
                    span,
                ));
            }
            if self.cx.names.controls.iter().any(|c| {
                c.name
                    == if lane.contains("::") {
                        lane.clone()
                    } else {
                        format!("{}::{lane}", info.name)
                    }
            }) {
                if clip.lane("notes").is_none()
                    && clip.scored.is_empty()
                    && clip.keys.is_none()
                    && !clip.signals.iter().any(|(l, _)| l == lane)
                {
                    return Err(Diagnostic::new(
                        format!("lane `{}` sets a parameter of node `{}` as each note starts, but clip `{}` has no notes", crate::compile::written(lane), info.name, clip.name),
                        span,
                    )
                    .with_help("add a `notes` lane to step on, or change it as the control instead"));
                }
                continue;
            }
            if let Some((fx, param)) = lane.split_once("::") {
                let params: Vec<&str> = self
                    .cx
                    .names
                    .controls
                    .iter()
                    .filter_map(|c| c.name.strip_prefix(&format!("{fx}::")))
                    .collect();
                let mut d = Diagnostic::new(
                    format!(
                        "clip `{}` has a lane `{fx}.{param}`, but there is no effect parameter of that name",
                        clip.name
                    ),
                    span,
                );
                if self.cx.lenient {
                    self.cx.skipped.borrow_mut().push(crate::compile::Skipped {
                        what: format!("lane `{fx}.{param}` of clip `{}`", clip.name),
                        name: lane.clone(),
                        span,
                        typo_when_new: false,
                    });
                    continue;
                }
                if let Some(c) = closest(param, params) {
                    d = d.with_help(format!("did you mean `{c}`?"));
                }
                return Err(d);
            }
            let mut known: Vec<&str> = instrument.params.iter().map(|p| p.name.as_str()).collect();
            known.extend(UNIVERSAL_LANES);
            let mut d = Diagnostic::new(
                format!(
                    "clip `{}` has a lane `{}`, but `{}` has no such parameter",
                    clip.name,
                    crate::compile::written(lane),
                    instrument.name
                ),
                span,
            );
            if self.cx.lenient {
                self.cx.skipped.borrow_mut().push(crate::compile::Skipped {
                    what: format!(
                        "lane `{}` of clip `{}`",
                        crate::compile::written(lane),
                        clip.name
                    ),
                    name: crate::compile::written(lane).to_string(),
                    span,
                    typo_when_new: false,
                });
                continue;
            }
            if let Some(c) = closest(lane, known) {
                d = d.with_help(format!("did you mean `{c}`?"));
            }
            return Err(d);
        }
        Ok(())
    }
}
