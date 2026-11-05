//! Parsing top-level items: definitions, arrangements, parameters and top-level `if`/`for`.

use super::*;

impl Parser {
    // ---- items -------------------------------------------------------

    /// Whether the item starting here is a sound-server definition rather than sequencing.
    pub(super) fn starts_definition(&self) -> bool {
        match self.peek() {
            Tok::Ident(w) => {
                matches!(
                    w.as_str(),
                    "ctl"
                        | "instr"
                        | "mono"
                        | "bus"
                        | "buffer"
                        | "midi_out"
                        | "sample"
                        | "kit"
                        | "config"
                        | "part"
                        | "fx"
                        | "node"
                        | "control"
                        | "fn"
                        | "opcode"
                        | "const"
                ) || self.peek_at(1) == &Tok::Eq
            }
            _ => false,
        }
    }

    pub(super) fn item(&mut self) -> Res<Item> {
        if self.dsp && !self.starts_definition() {
            return Err(self
                .unexpected("a definition such as `instr`, `node` or `bus`")
                .with_help(
                    "the audio server takes DSP definitions only; sequencing belongs to the client",
                ));
        }
        if self.is_keyword("on")
            && matches!(self.peek_at(1), Tok::Ident(s) if s == "midi" || s == "osc_in")
        {
            self.on()
        } else if self.is_keyword("ctl") {
            self.bump();
            let name = self.ident("a control name")?;
            self.expect(&Tok::Eq)?;
            if self.is_keyword("midi") || self.is_keyword("osc_in") {
                if self.dsp {
                    return Err(Diagnostic::new(
                        "the audio server accepts only DSP definitions; sequencing and musical timing belong to the client",
                        self.span(),
                    ));
                }
                return self.input_control(name);
            }
            Ok(Item::Control {
                name,
                value: self.expr()?,
                input: None,
            })
        } else if self.is_keyword("instr") || self.is_keyword("mono") {
            let mono = self.is_keyword("mono");
            let (name, params, options, body, span) = self.definition()?;
            Ok(Item::Instr(InstrDef {
                name,
                params,
                body,
                span,
                mono,
                options,
            }))
        } else if self.is_keyword("bus") {
            self.bump();
            let name = self.ident("a bus name")?;
            let layout = self.eat(&Tok::Colon).then(|| self.expr()).transpose()?;
            Ok(Item::Bus(BusDecl { name, layout }))
        } else if self.is_keyword("buffer") {
            let start = self.bump().span;
            let name = self.ident("a buffer name")?;
            self.expect(&Tok::Eq)?;
            let value = self.expr()?;
            Ok(Item::Buffer(BufferDecl {
                name,
                value,
                span: start.to(self.prev_span()),
            }))
        } else if self.is_keyword("midi_out") {
            let start = self.bump().span;
            let name = self.ident("a name for the MIDI output")?;
            let args = if self.peek() == &Tok::LParen {
                self.args()?
            } else {
                Vec::new()
            };
            Ok(Item::MidiOut(MidiOutDecl {
                name,
                args,
                span: start.to(self.prev_span()),
            }))
        } else if (self.is_keyword("sample") || self.is_keyword("kit"))
            && matches!(self.peek_at(1), Tok::Ident(_))
        {
            let word = self.bump();
            let Tok::Ident(word_name) = word.tok else {
                unreachable!()
            };
            Err(Diagnostic::new(format!("`{word_name}` is no longer a keyword"), word.span)
                .with_help(format!("write `name = {word_name}(\"drums/kick.wav\")`; settings are chained, as in `.beats(4)`")))
        } else if self.is_keyword("arrangement") {
            let start = self.bump().span;
            let name = self.ident("an arrangement name")?;
            self.arrangement(name, start).map(Item::Arrangement)
        } else if self.is_keyword("config") {
            let start = self.bump().span;
            let (entries, quantize, tuning, end) = self.config_entries()?;
            Ok(Item::Config(ConfigDecl {
                entries,
                quantize,
                tuning,
                span: start.to(end),
            }))
        } else if self.is_keyword("part") {
            let start = self.bump().span;
            Err(Diagnostic::new("`part` has been removed", start)
                .with_help("write `clip x { instrument: y }` instead"))
        } else if self.is_keyword("fx") || self.is_keyword("node") {
            let fx = self.is_keyword("fx");
            let start = self.bump().span;
            if !fx && !self.dsp {
                return Err(Diagnostic::new("`node` is for the sound server", start)
                    .with_help("write `clip name { instrument: sound }` to play something"));
            }
            let name = self.ident(if fx { "an fx name" } else { "a node name" })?;
            // `node bass = pluck(bright: 0.4)` is `node bass { instrument = pluck(bright: 0.4) }`
            if !fx && self.eat(&Tok::Eq) {
                let value = self.expr()?;
                let end = self.prev_span();
                let key = Ident {
                    name: "instrument".into(),
                    span: start,
                };
                return Ok(Item::Node(NodeDecl {
                    name,
                    params: Vec::new(),
                    body: vec![Stmt::Bind { name: key, value }],
                    options: Vec::new(),
                    span: start.to(end),
                }));
            }
            let params = if self.peek() == &Tok::LParen {
                self.params()?
            } else {
                Vec::new()
            };
            if let Some(p) = params.iter().find(|p| p.default.is_none()) {
                return Err(Diagnostic::new(
                    format!("fx parameter `{}` needs a starting value", p.name.name),
                    p.name.span,
                )
                .with_help("for example `fx bass(tone: hz = 2khz)`"));
            }
            let (mut body, end) = self.block()?;
            let options = take_options(&mut body, NODE_OPTIONS);
            let decl = NodeDecl {
                name,
                params,
                body,
                options,
                span: start.to(end),
            };
            Ok(if fx { Item::Fx(decl) } else { Item::Node(decl) })
        } else if self.is_keyword("control")
            && matches!(self.peek_at(1), Tok::Ident(w) if w == "fn")
        {
            // `control fn f(..) { body }` is `fn f(..) { control { body } }`
            let start = self.bump().span;
            if !matches!(self.peek_at(1), Tok::Ident(_)) {
                self.bump();
                return Err(self.unexpected("a function name"));
            }
            let mut def = self.function()?;
            if matches!(def.result, FnResult::Named(_)) {
                return Err(Diagnostic::new(
                    "a `control fn` has one result",
                    def.name.span,
                ));
            }
            let body_span = def.span;
            let lambda = Expr {
                kind: ExprKind::Lambda {
                    params: Vec::new(),
                    body: std::mem::take(&mut def.body),
                },
                span: body_span,
            };
            def.body = vec![Stmt::Expr(Expr {
                kind: ExprKind::Call {
                    name: Ident {
                        name: "control".into(),
                        span: start,
                    },
                    args: vec![Arg {
                        name: None,
                        receiver: false,
                        value: lambda,
                    }],
                },
                span: body_span,
            })];
            def.span = start.to(def.span);
            Ok(Item::Fn(def))
        } else if self.is_keyword("fn") {
            if !matches!(self.peek_at(1), Tok::Ident(_)) {
                self.bump();
                return Err(self.unexpected("a function name"));
            }
            self.function().map(Item::Fn)
        } else if self.is_keyword("def") {
            if !matches!(self.peek_at(1), Tok::Ident(_)) {
                self.bump();
                return Err(self.unexpected("a score function name"));
            }
            // same shape as `fn`, but lives in the performance layer
            self.function_as_def()
        } else if self.is_keyword("for") && matches!(self.peek_at(1), Tok::Ident(_)) {
            self.top_for()
        } else if self.is_keyword("if") {
            self.top_if()
        } else if self.is_keyword("opcode") {
            Err(
                Diagnostic::new("`opcode` is now `fn`", self.span()).with_help(
                    "write `fn name(x, ...) { ... }`; a function with `state` is a unit generator",
                ),
            )
        } else if self.is_keyword("const") {
            self.bump();
            let name = self.ident("a name for the constant")?;
            self.expect(&Tok::Eq)?;
            Ok(Item::Const {
                name,
                value: self.expr()?,
            })
        } else if self.is_keyword("tempo") {
            let start = self.bump().span;
            let value = self.expr()?;
            let quantize = self.quantize()?;
            let over = if self.is_keyword("over") {
                self.bump();
                Some(self.expr()?)
            } else {
                None
            };
            Ok(Item::Tempo(TempoStmt {
                value,
                quantize,
                over,
                span: start.to(self.prev_span()),
            }))
        } else if self.is_keyword("meter") {
            self.bump();
            Ok(Item::Meter(self.expr()?))
        } else if self.is_keyword("clip") || self.is_keyword("scene") {
            let is_clip = self.is_keyword("clip");
            let start = self.bump().span;
            let name = self.ident(if is_clip {
                "a clip name"
            } else {
                "a scene name"
            })?;
            // `clip bass_chorus = bass.transpose(+5st)`
            if is_clip && self.eat(&Tok::Eq) {
                let base = self.expr()?;
                let block = NamedBlock {
                    name,
                    base: Some(Box::new(base)),
                    entries: Vec::new(),
                    options: Vec::new(),
                    span: start.to(self.prev_span()),
                };
                return Ok(Item::Clip(block));
            }
            let (mut entries, end) = self.entries(!is_clip)?;
            let options = take_entry_options(&mut entries);
            let block = NamedBlock {
                name,
                base: None,
                entries,
                options,
                span: start.to(end),
            };
            Ok(if is_clip {
                Item::Clip(block)
            } else {
                Item::Scene(block)
            })
        } else if self.is_keyword("macro") && matches!(self.peek_at(1), Tok::Ident(_)) {
            let start = self.bump().span;
            let name = self.ident("a macro name")?;
            let (entries, end) = self.entries(false)?;
            Ok(Item::Macro(NamedBlock {
                name,
                base: None,
                entries,
                options: Vec::new(),
                span: start.to(end),
            }))
        } else if self.is_keyword("at") {
            self.bump();
            let at = self.at_pos()?;
            if let AtPos::Marker { section, .. } = &at {
                return Err(Diagnostic::new(
                    format!("`{}.start` and `.end` are only known inside an arrangement with `sections`", section.name),
                    section.span,
                ));
            }
            let at_bar = matches!(at, AtPos::Bar { .. });
            if self.at_command() || self.is_edit() {
                if at_bar {
                    return Err(Diagnostic::new(
                        "a command can't be placed at a bar number, which counts on a clock that keeps running: write `at +4bars` instead",
                        self.span(),
                    ));
                }
                if self.at_command() {
                    self.command(Some(at)).map(Item::Command)
                } else {
                    self.edit(Some(at)).map(Item::Command)
                }
            } else {
                self.note(Some(at)).map(Item::Note)
            }
        } else if self.at_command() {
            self.command(None).map(Item::Command)
        } else if self.is_edit() {
            self.edit(None).map(Item::Command)
        } else if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Eq {
            let name = self.ident("a name")?;
            self.bump();
            if self.is_keyword("kit") && self.peek_at(1) == &Tok::LBrace {
                self.bump();
                let start = self.bump().span;
                let mut entries = Vec::new();
                self.skip_newlines();
                while self.peek() != &Tok::RBrace {
                    let entry = self.ident("a kit member name")?;
                    self.expect(&Tok::Colon)?;
                    entries.push((entry, self.expr()?));
                    if self.peek() != &Tok::RBrace
                        && !self.eat(&Tok::Comma)
                        && self.peek() != &Tok::Newline
                    {
                        return Err(self.unexpected("a newline, comma, or `}`"));
                    }
                    self.skip_newlines();
                }
                let end = self.bump().span;
                let decl = SampleDecl {
                    span: name.span.to(end),
                    name,
                    is_kit: true,
                    source: SampleSource::Instruments(entries),
                    source_span: start.to(end),
                    options: Vec::new(),
                };
                return Ok(Item::Sample(decl));
            }
            let value = self.expr()?;
            if self.dsp || is_sample_chain(&value) {
                sample_decl(name, value)
            } else {
                Ok(Item::Bind { name, value })
            }
        } else {
            self.note(None).map(Item::Note)
        }
    }

    /// The body of `arrangement name { sections: [scene.over(8bars) ...] }`.
    pub(super) fn arrangement(&mut self, name: Ident, start: Span) -> Res<ArrangementDecl> {
        self.expect(&Tok::LBrace)?;
        let mut sections = None;
        let (mut tempos, mut meters) = (None, None);
        let mut tempo_env: Option<EnvLit> = None;
        let mut lanes: Vec<ArrangementLane> = Vec::new();
        let mut commands: Vec<CommandStmt> = Vec::new();
        loop {
            self.skip_newlines();
            if self.peek() == &Tok::RBrace {
                break;
            }
            if self.is_keyword("at") {
                self.bump();
                let at = self.at_pos()?;
                if matches!(at, AtPos::Bar { .. }) || !(self.at_command() || self.is_edit()) {
                    return Err(self.unexpected(
                        "a command placed with `at +4bars`, such as `at +4bars chorus.start`",
                    ));
                }
                let command = if self.at_command() {
                    self.command(Some(at))?
                } else {
                    self.edit(Some(at))?
                };
                commands.push(command);
                continue;
            }
            let key = self.ident("`sections`")?;
            if self.peek() == &Tok::Dot {
                self.bump();
                let param = self.ident("a parameter name")?;
                self.expect(&Tok::Colon)?;
                let value = self.expr()?;
                let ExprKind::Env(lit) = value.kind else {
                    return Err(
                        Diagnostic::new("an arrangement lane is an envelope", value.span)
                            .with_help(format!(
                                "for example `{}.{}: env[0.2 8bars 1.0]`",
                                key.name, param.name
                            )),
                    );
                };
                if lanes
                    .iter()
                    .any(|l| l.node.name == key.name && l.param.name == param.name)
                {
                    return Err(Diagnostic::new(
                        format!("lane `{}.{}` is given twice", key.name, param.name),
                        key.span,
                    ));
                }
                lanes.push(ArrangementLane {
                    node: key,
                    param,
                    value: lit,
                });
                if !matches!(self.peek(), Tok::Newline | Tok::RBrace) {
                    return Err(self.unexpected("the end of the line"));
                }
                continue;
            }
            if key.name == "tempo" || key.name == "meter" {
                self.expect(&Tok::Colon)?;
                let slot = if key.name == "tempo" {
                    &mut tempos
                } else {
                    &mut meters
                };
                if slot.is_some() || (key.name == "tempo" && tempo_env.is_some()) {
                    return Err(Diagnostic::new(
                        format!("`{}` is given twice", key.name),
                        key.span,
                    ));
                }
                if key.name == "tempo" && self.is_keyword("env") {
                    let value = self.expr()?;
                    let ExprKind::Env(lit) = value.kind else {
                        unreachable!("`env` starts an envelope")
                    };
                    tempo_env = Some(lit);
                    if !matches!(self.peek(), Tok::Newline | Tok::RBrace) {
                        return Err(self.unexpected("the end of the line"));
                    }
                    continue;
                }
                self.expect(&Tok::LBracket)?;
                let mut list = Vec::new();
                while self.peek() != &Tok::RBracket {
                    list.push(self.expr()?);
                    self.eat(&Tok::Comma);
                }
                self.expect(&Tok::RBracket)?;
                *slot = Some(list);
                if !matches!(self.peek(), Tok::Newline | Tok::RBrace) {
                    return Err(self.unexpected("the end of the line"));
                }
                continue;
            }
            if key.name != "sections" {
                return Err(Diagnostic::new(format!("`{}` isn't supported in an arrangement yet", key.name), key.span)
                    .with_help("an arrangement has `at +4bars chorus.start` lines, or `sections`, with `tempo` and `meter` for each of them, and envelope lanes such as `bass.tone: env[...]`"));
            }
            self.expect(&Tok::Colon)?;
            if sections.is_some() {
                return Err(Diagnostic::new("`sections` is given twice", key.span));
            }
            self.expect(&Tok::LBracket)?;
            let mut list = Vec::new();
            while self.peek() != &Tok::RBracket {
                let scene = self.ident("a scene name")?;
                if !(self.eat(&Tok::Dot) && self.is_keyword("over")) {
                    return Err(Diagnostic::new(
                        format!("`{}` needs a length", scene.name),
                        scene.span,
                    )
                    .with_help(format!("for example `{}.over(8bars)`", scene.name)));
                }
                self.bump();
                self.expect(&Tok::LParen)?;
                let length = self.expr()?;
                self.expect(&Tok::RParen)?;
                let label = if self.is_keyword("as") {
                    self.bump();
                    Some(self.ident("a name for this section")?)
                } else {
                    None
                };
                list.push(SectionRef {
                    scene,
                    length,
                    label,
                });
                self.eat(&Tok::Comma);
            }
            self.expect(&Tok::RBracket)?;
            sections = Some(list);
            if !matches!(self.peek(), Tok::Newline | Tok::RBrace) {
                return Err(self.unexpected("the end of the line"));
            }
        }
        let end = self.expect(&Tok::RBrace)?;
        if sections.is_none() && commands.is_empty() {
            return Err(Diagnostic::new(
                format!("arrangement `{}` has nothing in it", name.name),
                name.span,
            )
            .with_help(
                "add `at +0bars verse.start` lines, or `sections: [scene.over(8bars) ...]`",
            ));
        }
        let sections = sections.unwrap_or_default();
        Ok(ArrangementDecl {
            name,
            sections,
            commands,
            tempos,
            tempo_env,
            meters,
            lanes,
            span: start.to(end),
        })
    }

    /// `(a: hz = 1khz, b, size: const int = 4096)`
    pub(super) fn params(&mut self) -> Res<Vec<ParamDef>> {
        self.expect(&Tok::LParen)?;
        let mut params = Vec::new();
        while self.peek() != &Tok::RParen {
            let pname = self.ident("a parameter name")?;
            let (mut rate, mut ty) = (None, None);
            if self.eat(&Tok::Colon) {
                if let Tok::Ident(word) = self.peek().clone() {
                    let r = match word.as_str() {
                        "const" => Some(Rate::Const),
                        "init" => Some(Rate::Init),
                        "control" => Some(Rate::Control),
                        _ => None,
                    };
                    if let Some(r) = r {
                        rate = Some((r, self.bump().span));
                    }
                }
                if rate.is_none() || !matches!(self.peek(), Tok::Eq | Tok::Comma | Tok::RParen) {
                    ty = Some(self.type_spec()?);
                }
            }
            let default = self.eat(&Tok::Eq).then(|| self.expr()).transpose()?;
            params.push(ParamDef {
                name: pname,
                rate,
                ty,
                default,
            });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen)?;
        Ok(params)
    }

    /// `instr name(params) { settings and body }`
    pub(super) fn definition(&mut self) -> Res<Definition> {
        let start = self.bump().span;
        let name = self.ident("an instrument name")?;
        let params = self.params()?;
        if self.peek() == &Tok::Arrow {
            return Err(Diagnostic::new(
                "an instrument's result is a signal, so it has no `->`",
                self.span(),
            )
            .with_help("functions name their results with `->`"));
        }
        if self.is_keyword("with") {
            return Err(with_gone(self.span()));
        }
        let (mut body, end) = self.block()?;
        let options = take_options(&mut body, INSTR_OPTIONS);
        Ok((name, params, options, body, start.to(end)))
    }

    /// `fn name(params) -> result { body }`
    pub(super) fn function_as_def(&mut self) -> Res<Item> {
        let mut def = self.function_from("def")?;
        // score functions don't declare DSP result units
        def.result = FnResult::Inferred;
        Ok(Item::Def(def))
    }

    pub(super) fn item_block(&mut self) -> Res<(Vec<Item>, Span)> {
        self.expect(&Tok::LBrace)?;
        let mut body = Vec::new();
        loop {
            self.skip_newlines();
            if self.peek() == &Tok::RBrace {
                break;
            }
            body.push(self.item()?);
            self.skip_newlines();
        }
        let end = self.expect(&Tok::RBrace)?;
        Ok((body, end))
    }

    pub(super) fn top_if(&mut self) -> Res<Item> {
        let start = self.bump().span; // `if`
        let cond = self.expr()?;
        let (then, then_end) = self.item_block()?;
        let mut otherwise = Vec::new();
        let mut end = then_end;
        self.skip_newlines();
        if self.is_keyword("else") {
            self.bump();
            self.skip_newlines();
            if self.is_keyword("if") {
                // `else if` as a nested top-level if
                let nested = self.top_if()?;
                end = match &nested {
                    Item::If { span, .. } => *span,
                    _ => self.prev_span(),
                };
                otherwise.push(nested);
            } else {
                let (body, else_end) = self.item_block()?;
                otherwise = body;
                end = else_end;
            }
        }
        Ok(Item::If {
            cond,
            then,
            otherwise,
            span: start.to(end),
        })
    }

    pub(super) fn top_for(&mut self) -> Res<Item> {
        let start = self.bump().span; // `for`
        let var = self.ident("the loop variable")?;
        if !self.is_keyword("in") {
            return Err(self.unexpected("`in`"));
        }
        self.bump();
        let iter = self.expr()?;
        self.expect(&Tok::LBrace)?;
        let mut body = Vec::new();
        loop {
            self.skip_newlines();
            if self.peek() == &Tok::RBrace {
                break;
            }
            body.push(self.item()?);
            self.skip_newlines();
        }
        let end = self.expect(&Tok::RBrace)?;
        Ok(Item::For {
            var,
            iter,
            body,
            span: start.to(end),
        })
    }
}
