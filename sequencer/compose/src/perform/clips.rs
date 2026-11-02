//! Building clips and scenes.

use super::*;

impl Perform<'_> {
    // ---- clips and scenes ------------------------------------------------------

    pub(super) fn clip(&self, block: &NamedBlock) -> Res<Clip> {
        if let Some(base) = &block.base {
            return match self.eval(base, None)? {
                PerfValue::Clip(mut clip) => {
                    clip.name = block.name.name.clone();
                    Ok(clip)
                }
                other => Err(Diagnostic::new(
                    format!(
                        "a clip is made from another clip, but this is {}",
                        other.describe()
                    ),
                    base.span,
                )
                .with_help("for example `clip bass_chorus = bass.transpose(+5st)`")),
            };
        }
        let mut length = None;
        let mut keys = None;
        let mut lanes: Vec<(String, Pattern<Step>)> = Vec::new();
        let mut curves: Vec<(String, EnvLane)> = Vec::new();
        let mut scored = Vec::new();
        let mut plays = None;
        let mut then = Vec::new();
        // `fx_name { open: x.x. }` is the lanes `fx_name.open`, written together
        let mut entries: Vec<(Ident, &Expr, bool)> = Vec::new();
        for (key, value) in &block.entries {
            if let ExprKind::FxLanes(rows) = &value.kind {
                for (row, row_value) in rows {
                    let name = format!("{}::{}", key.name, row.name);
                    entries.push((
                        Ident {
                            name,
                            span: row.span,
                        },
                        row_value,
                        true,
                    ));
                }
            } else {
                entries.push((key.clone(), value, false));
            }
        }
        for (key, value, in_block) in entries {
            if key.name == "keys" {
                if keys.is_some() {
                    return Err(Diagnostic::new("lane `keys` is given twice", key.span));
                }
                keys = Some(self.keys(value)?);
                continue;
            }
            if key.name == "length" {
                let cycles = self.cycles(value)?;
                if cycles <= Rational::ZERO {
                    return Err(Diagnostic::new(
                        "a clip needs a length above zero",
                        value.span,
                    ));
                }
                length = Some(cycles);
                continue;
            }
            if key.name == "times" {
                if plays.is_some() {
                    return Err(Diagnostic::new("`times` is given twice", key.span));
                }
                plays = Some(self.whole(value, "times")? as u32);
                continue;
            }
            if key.name == "then" {
                then.push(self.then_command(value)?);
                continue;
            }
            if key.name == "events" {
                if !scored.is_empty() {
                    return Err(Diagnostic::new("lane `events` is given twice", key.span));
                }
                let ExprKind::Events(raw) = &value.kind else {
                    return Err(
                        Diagnostic::new("an `events` lane is a brace block", value.span).with_help(
                            "for example `events { 0beat c4 for 1/2beat  1/2beat e4 for 1/2beat }`",
                        ),
                    );
                };
                scored = parse_events(raw, value.span, self.cx)?;
                continue;
            }
            // a lane named for a macro moves everything the macro moves, each along its own range
            if !in_block && let Some(def) = self.cx.macros.iter().find(|m| m.name == key.name) {
                let found = |name: &str| {
                    lanes.iter().any(|(n, _)| n == name) || curves.iter().any(|(n, _)| n == name)
                };
                if let Some(target) = def.targets.iter().find(|t| found(&t.control)) {
                    return Err(Diagnostic::new(
                        format!(
                            "macro `{}` moves `{}`, which this clip also has a lane for",
                            def.name,
                            crate::compile::written(&target.control)
                        ),
                        key.span,
                    ));
                }
                match self.eval(value, None)? {
                    PerfValue::Pattern(p) => self.fan_out(def, &p, &mut lanes),
                    PerfValue::Env(curve) => fan_out_env(def, &curve, &mut curves),
                    PerfValue::Num(v) => {
                        self.fan_out(def, &Pattern::pure(Step::from(v)), &mut lanes)
                    }
                    PerfValue::List(items) => {
                        let p = self.list_pattern(items, value.span)?;
                        self.fan_out(def, &p, &mut lanes);
                    }
                    other => {
                        return Err(Diagnostic::new(
                            format!(
                                "a macro lane holds levels from 0 to 1, but this is {}",
                                other.describe()
                            ),
                            value.span,
                        ));
                    }
                }
                continue;
            }
            if lanes.iter().any(|(n, _)| *n == key.name)
                || curves.iter().any(|(n, _)| *n == key.name)
            {
                return Err(Diagnostic::new(
                    format!(
                        "lane `{}` is given twice",
                        crate::compile::written(&key.name)
                    ),
                    key.span,
                ));
            }
            // in an effect's block a bracketed row of steps, `[x . <x X .>]`, is read as steps
            if in_block
                && let ExprKind::Pattern(raw) = &value.kind
                && let Ok(pattern) = mini_steps(raw)
            {
                lanes.push((key.name.clone(), pattern));
                continue;
            }
            let pattern = match self.eval(value, None)? {
                PerfValue::Pattern(p) => p,
                PerfValue::Env(curve) => {
                    if key.name == "notes" {
                        return Err(Diagnostic::new(
                            "the `notes` lane holds notes, not an envelope",
                            value.span,
                        )
                        .with_help(
                            "an envelope moves a node's parameter: `cut: env[300hz 1bar 3khz]`",
                        ));
                    }
                    curves.push((key.name.clone(), curve));
                    continue;
                }
                PerfValue::Num(v) => Pattern::pure(Step::from(v)),
                PerfValue::Rest => Pattern::silence(),
                PerfValue::List(items) => self.list_pattern(items, value.span)?,
                other => {
                    return Err(Diagnostic::new(
                        format!("a lane holds a pattern, but this is {}", other.describe()),
                        value.span,
                    ));
                }
            };
            lanes.push((key.name.clone(), pattern));
        }
        if !scored.is_empty() && lanes.iter().any(|(n, _)| n == "notes") {
            return Err(Diagnostic::new(
                "a clip can't have both `notes` and `events`",
                block.span,
            ));
        }
        // a clip with rows for an effect and no notes is a gesture: a silent note on every sixteenth gives
        // the rows a step to land on
        let macro_names: Vec<&str> = self.cx.macros.iter().map(|m| m.name.as_str()).collect();
        let rows_only = crate::dsp::drives_effects(block, &macro_names)
            && !block.entries.iter().any(|(k, _)| k.name == "instrument");
        if rows_only
            && keys.is_none()
            && scored.is_empty()
            && !lanes.iter().any(|(n, _)| n == "notes")
        {
            let steps = 16 * length.map_or(1, |l| l.to_f64().ceil().max(1.0) as usize);
            let step = Pattern::pure(Step::from(niminal_score::Value::Note(60)));
            lanes.push(("notes".into(), Pattern::fastcat(vec![step; steps])));
        }
        // a lane spans the whole clip, whatever its length
        if let Some(length) = length {
            lanes = lanes
                .into_iter()
                .map(|(name, p)| (name, p.slow(length)))
                .collect();
        }
        let endless = length.is_none() && plays.is_none();
        Ok(Clip {
            name: block.name.name.clone(),
            length: length.unwrap_or(Rational::ONE),
            endless,
            plays,
            then,
            lanes,
            scored,
            keys,
            curves,
            signals: Vec::new(),
            fx_params: Vec::new(),
        })
    }

    /// A macro's lane as the lanes of everything it moves: each level from 0 to 1 becomes where that control is
    /// at that level.
    pub(super) fn fan_out(
        &self,
        def: &crate::compile::MacroDef,
        pattern: &Pattern<Step>,
        lanes: &mut Vec<(String, Pattern<Step>)>,
    ) {
        for target in &def.targets {
            let target = target.clone();
            lanes.push((
                target.control.clone(),
                pattern.map(move |step| {
                    let level = step.value.as_number().unwrap_or(0.0);
                    step.with(macro_value(&target, level))
                }),
            ));
        }
    }

    /// `then: chorus.start`: what to do when a clip has played its last time, as the statement to run.
    pub(super) fn then_command(&self, e: &Expr) -> Res<String> {
        const VERBS: [&str; 6] = ["start", "stop", "mute", "unmute", "solo", "unsolo"];
        match &e.kind {
            ExprKind::Name(n) if n == "hush" => return Ok("hush".to_string()),
            ExprKind::Call { name, args } if VERBS.contains(&name.name.as_str()) => {
                if let [receiver] = args.as_slice()
                    && receiver.receiver
                    && let ExprKind::Name(target) = &receiver.value.kind
                {
                    return Ok(format!("{target}.{}", name.name));
                }
            }
            _ => {}
        }
        Err(
            Diagnostic::new("`then` takes a command on a clip or scene", e.span)
                .with_help("for example `then: chorus.start` or `then: hush`"),
        )
    }

    /// `midi.keys()` or `midi.keys(channel: 1)`
    pub(super) fn keys(&self, e: &Expr) -> Res<Keys> {
        let ExprKind::Call { name, args } = &e.kind else {
            return Err(self.keys_error(e.span));
        };
        let from_midi = matches!(args.first(), Some(Arg { receiver: true, value: Expr { kind: ExprKind::Name(m), .. }, .. }) if m == "midi");
        if name.name != "keys" || !from_midi {
            return Err(self.keys_error(e.span));
        }
        let mut channel = None;
        let mut mpe = false;
        for arg in &args[1..] {
            match arg.name.as_ref().map(|n| n.name.as_str()) {
                Some("mpe") => match &arg.value.kind {
                    ExprKind::Name(n) if n == "true" => mpe = true,
                    ExprKind::Name(n) if n == "false" => mpe = false,
                    _ => {
                        return Err(Diagnostic::new(
                            "`mpe` is `true` or `false`",
                            arg.value.span,
                        ));
                    }
                },
                Some("channel") => {
                    let n = self.plain(&arg.value)?;
                    if !n.is_integer() || n < Rational::ONE || n > Rational::int(16) {
                        return Err(Diagnostic::new(
                            "a MIDI channel is a whole number from 1 to 16",
                            arg.value.span,
                        ));
                    }
                    channel = Some(n.numer() as u8);
                }
                _ => {
                    return Err(Diagnostic::new(
                        "`midi.keys` takes `channel:` or `mpe:`",
                        arg.value.span,
                    ));
                }
            }
        }
        if mpe && channel.is_some() {
            return Err(Diagnostic::new(
                "`mpe: true` plays from every member channel, so it takes no `channel:`",
                e.span,
            ));
        }
        Ok(Keys { channel, mpe })
    }

    pub(super) fn keys_error(&self, span: Span) -> Diagnostic {
        Diagnostic::new(
            "`keys` plays a clip from a keyboard: write `keys: midi.keys(channel: 1)`",
            span,
        )
    }

    pub(super) fn scene(&self, block: &NamedBlock) -> Res<Scene> {
        let mut clips: Vec<usize> = Vec::new();
        for (key, value) in &block.entries {
            if !matches!(&value.kind, ExprKind::Name(n) if *n == key.name) {
                return Err(Diagnostic::new(
                    format!("a scene is a list of clips, so `{}` has nothing after it", key.name),
                    value.span,
                )
                .with_help(
                    "write `scene verse { bass, lead }`; for a different bass in this scene, declare another clip",
                ));
            }
            let node = self.node(key)?;
            if clips.contains(&node) {
                return Err(Diagnostic::new(
                    format!("clip `{}` appears twice in this scene", key.name),
                    key.span,
                ));
            }
            clips.push(node);
        }
        Ok(Scene {
            name: block.name.name.clone(),
            clips,
        })
    }

    /// Anything playable as a clip: a clip, or a bare pattern of notes.
    pub(super) fn to_clip(&self, v: PerfValue, hint: &str, span: Span) -> Res<Clip> {
        match v {
            PerfValue::Clip(c) => Ok(c),
            PerfValue::Pattern(p) => Ok(Clip {
                name: hint.to_string(),
                length: Rational::ONE,
                endless: true,
                plays: None,
                then: Vec::new(),
                lanes: vec![("notes".to_string(), p)],
                scored: Vec::new(),
                keys: None,
                curves: Vec::new(),
                signals: Vec::new(),
                fx_params: Vec::new(),
            }),
            PerfValue::Num(v) => Ok(Clip {
                name: hint.to_string(),
                length: Rational::ONE,
                endless: true,
                plays: None,
                then: Vec::new(),
                lanes: vec![("notes".to_string(), Pattern::pure(Step::from(v)))],
                scored: Vec::new(),
                keys: None,
                curves: Vec::new(),
                signals: Vec::new(),
                fx_params: Vec::new(),
            }),
            PerfValue::Grid(_) => Err(Diagnostic::new("a grid has no pitches to play", span)
                .with_help("give it notes with `.rhythm(grid[...])` on a pattern instead")),
            PerfValue::Scene(_) => Err(Diagnostic::new("a scene is launched, not played", span)
                .with_help("write `name.start`")),
            PerfValue::Env(_) => Err(Diagnostic::new(
                "an envelope moves a parameter, so it goes in a lane",
                span,
            )
            .with_help("for example `clip c { notes: [c4] cut: env[300hz 1bar 3khz] }`")),
            PerfValue::Rest => Err(Diagnostic::new(
                "`~` stops a clip in a scene; to stop one, write `node.stop`",
                span,
            )),
            PerfValue::List(_) => Err(Diagnostic::new(
                "a list isn't a clip; play one entry, or build a clip from it",
                span,
            )),
            PerfValue::Def(d) => Err(Diagnostic::new(
                format!(
                    "`{}` is a score function; call it, as in `{}(...)`",
                    d.name, d.name
                ),
                span,
            )),
        }
    }
}
