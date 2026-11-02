//! Parsing `on` handlers and their input sources (MIDI and OSC).

use super::*;

impl Parser {
    /// `midi.cc(21)`, `midi.note(36)`, `midi.learn` or `osc_in("/fx/width")`.
    pub(super) fn input_source(&mut self) -> Res<InputSource> {
        let start = self.span();
        Ok(if self.is_keyword("midi") {
            self.bump();
            self.expect(&Tok::Dot)?;
            let method = self.ident("`cc`, `note` or `learn`")?;
            match method.name.as_str() {
                "learn" => InputSource::MidiLearn,
                "cc" | "note" => {
                    let what = if method.name == "cc" {
                        "a CC number from 0 to 127"
                    } else {
                        "a note number from 0 to 127"
                    };
                    self.expect(&Tok::LParen)?;
                    // a key can be given by name: `midi.note(c1)`
                    let named = match self.peek().clone() {
                        Tok::Ident(n) if method.name == "note" => {
                            match n.parse::<niminal_score::Value>() {
                                Ok(niminal_score::Value::Note(m)) if (0..=127).contains(&m) => {
                                    self.bump();
                                    Some(m as u8)
                                }
                                _ => return Err(self.unexpected(what)),
                            }
                        }
                        _ => None,
                    };
                    let number = match named {
                        Some(n) => n,
                        None => self.small_number(what, 127)?,
                    };
                    let channel = if self.eat(&Tok::Comma) {
                        Some(self.small_number("a channel from 1 to 16", 16)?)
                    } else {
                        None
                    };
                    if channel == Some(0) {
                        return Err(Diagnostic::new(
                            "MIDI channels count from 1",
                            start.to(self.prev_span()),
                        ));
                    }
                    self.expect(&Tok::RParen)?;
                    if method.name == "cc" {
                        InputSource::MidiCc {
                            cc: number,
                            channel,
                        }
                    } else {
                        InputSource::MidiNote {
                            note: number,
                            channel,
                        }
                    }
                }
                other => {
                    return Err(Diagnostic::new(
                        format!(
                            "`midi.{other}` isn't supported; use `midi.cc(number)`, `midi.note(number)` or `midi.learn`"
                        ),
                        method.span,
                    ));
                }
            }
        } else {
            self.bump();
            self.expect(&Tok::LParen)?;
            let Tok::Str(address) = self.peek().clone() else {
                return Err(self.unexpected("an OSC address such as \"/fx/width\""));
            };
            self.bump();
            if !address.starts_with('/') {
                return Err(Diagnostic::new(
                    "an OSC address starts with `/`",
                    start.to(self.prev_span()),
                ));
            }
            self.expect(&Tok::RParen)?;
            InputSource::Osc(address)
        })
    }

    /// `on midi.note(36) beat.start`, or several commands in braces.
    pub(super) fn on(&mut self) -> Res<Item> {
        let start = self.span();
        self.bump();
        if !(self.is_keyword("midi") || self.is_keyword("osc_in")) {
            return Err(self.unexpected("`midi.note(36)`, `midi.cc(64)` or `osc_in(\"/address\")`"));
        }
        let source_start = self.span();
        let source = self.input_source()?;
        if source == InputSource::MidiLearn {
            return Err(Diagnostic::new(
                "`midi.learn` is for a control; name the note or controller for an `on`",
                source_start.to(self.prev_span()),
            ));
        }
        let mut commands = Vec::new();
        let mut command = |p: &mut Parser| -> Res<()> {
            let first = p.span();
            p.expr()?;
            commands.push(first.to(p.prev_span()));
            Ok(())
        };
        if self.eat(&Tok::LBrace) {
            loop {
                self.skip_newlines();
                if self.eat(&Tok::RBrace) {
                    break;
                }
                command(self)?;
            }
        } else {
            command(self)?;
        }
        Ok(Item::On(OnDecl {
            source,
            commands,
            span: start.to(self.prev_span()),
        }))
    }

    /// The rest of `ctl name = midi.cc(21).range(200hz..4khz).smooth(10ms)`,
    /// `ctl name = osc_in("/fx/width").range(0..1)` or `ctl held = midi.note(36)`, which is open while the
    /// key is down.
    pub(super) fn input_control(&mut self, name: Ident) -> Res<Item> {
        let start = self.span();
        let source = self.input_source()?;
        let number = |value: f64, span| Expr {
            kind: ExprKind::Num { value, unit: None },
            span,
        };
        let (mut lo, mut hi, mut smooth, mut quantize) =
            (number(0.0, start), number(1.0, start), None, None);
        while self.eat(&Tok::Dot) {
            let method = self.ident("`range`, `smooth` or `quantize`")?;
            self.expect(&Tok::LParen)?;
            match method.name.as_str() {
                "range" => {
                    let range = self.expr()?;
                    let ExprKind::Range {
                        lo: l,
                        hi: h,
                        inclusive: true,
                    } = range.kind
                    else {
                        return Err(Diagnostic::new(
                            "`range` takes a range, such as `200hz..4khz`",
                            range.span,
                        ));
                    };
                    lo = *l;
                    hi = *h;
                }
                "smooth" => smooth = Some(self.expr()?),
                "quantize" => quantize = Some(self.expr()?),
                other => {
                    return Err(Diagnostic::new(
                        format!(
                            "a control input has no `.{other}`; use `.range`, `.smooth` or `.quantize`"
                        ),
                        method.span,
                    ));
                }
            }
            self.expect(&Tok::RParen)?;
        }
        let value = match smooth {
            None => lo,
            Some(duration) => {
                let span = lo.span.to(duration.span);
                let args = vec![
                    Arg {
                        name: None,
                        value: lo,
                        receiver: true,
                    },
                    Arg {
                        name: None,
                        value: duration,
                        receiver: false,
                    },
                ];
                Expr {
                    kind: ExprKind::Call {
                        name: Ident {
                            name: "smooth".into(),
                            span,
                        },
                        args,
                    },
                    span,
                }
            }
        };
        Ok(Item::Control {
            name,
            value,
            input: Some(ControlInput {
                source,
                hi,
                quantize,
            }),
        })
    }

    pub(super) fn small_number(&mut self, what: &str, max: u8) -> Res<u8> {
        match self.peek().clone() {
            Tok::Num { value, unit: None }
                if value.fract() == 0.0 && (0.0..=f64::from(max)).contains(&value) =>
            {
                self.bump();
                Ok(value as u8)
            }
            _ => Err(self.unexpected(what)),
        }
    }
}
