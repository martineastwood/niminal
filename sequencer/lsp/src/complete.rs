//! What to offer at the cursor.

use niminal_compose::{RESERVED_WORDS, Symbol, SymbolKind, pattern_ops};
use niminal_lang::{OpInfo, builtin_ops, waves};
use serde_json::{Value, json};
const UNITS: &[(&str, &str)] = &[
    ("hz", "frequency"), ("khz", "frequency, in thousands of hz"), ("db", "gain in decibels"), ("ms", "milliseconds"),
    ("sec", "seconds"), ("beat", "beats, at the tempo"), ("beats", "beats, at the tempo"), ("bar", "bars, at the tempo and meter"),
    ("bars", "bars, at the tempo and meter"), ("deg", "an angle in degrees"), ("st", "semitones"), ("ct", "cents, hundredths of a semitone"),
];

// LSP CompletionItemKind
const FUNCTION: u32 = 3;
const VARIABLE: u32 = 6;
const KEYWORD: u32 = 14;
const UNIT: u32 = 11;
const ENUM_MEMBER: u32 = 20;
const PROPERTY: u32 = 10;

fn item(label: &str, kind: u32, detail: &str, doc: &str) -> Value {
    let mut item = json!({ "label": label, "kind": kind, "detail": detail });
    if !doc.is_empty() {
        item["documentation"] = json!({ "kind": "markdown", "value": doc });
    }
    item
}

fn symbol_kind(kind: SymbolKind) -> u32 {
    match kind {
        SymbolKind::Instrument | SymbolKind::Function => FUNCTION,
        _ => VARIABLE,
    }
}

/// The call the cursor is inside, if any: its name, whether it was written `x.name(`,
/// the argument names already given, and how many arguments came before this one.
struct Call<'a> {
    name: &'a str,
    receiver: bool,
    /// The text before the `.` of `x.name(`.
    receiver_text: &'a str,
    named: Vec<&'a str>,
    unnamed_before: usize,
    /// The text of the argument being written, up to the cursor.
    current: &'a str,
}

fn enclosing_call(before: &str) -> Option<Call<'_>> {
    let window_start = before.len().saturating_sub(4000);
    let window_start = (window_start..=before.len()).find(|&i| before.is_char_boundary(i))?;
    let window = &before[window_start..];
    let mut depth = 0usize;
    let open = window.char_indices().rev().find_map(|(i, c)| match c {
        ')' | ']' => {
            depth += 1;
            None
        }
        '(' | '[' if depth == 0 => Some(i),
        '(' | '[' => {
            depth -= 1;
            None
        }
        _ => None,
    })?;
    if !window[open..].starts_with('(') {
        return None;
    }
    let head = &window[..open];
    let start = head.char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || *c == '_').last().map(|(i, _)| i)?;
    let name = &head[start..];
    let receiver = head[..start].trim_end().ends_with('.');
    let receiver_text = head[..start].trim_end().strip_suffix('.').unwrap_or_default();

    let mut named = Vec::new();
    let mut unnamed_before = 0;
    let mut segment_start = open + 1;
    let mut depth = 0usize;
    let args = &window[open + 1..];
    let mut segments = Vec::new();
    for (i, c) in args.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                segments.push(&args[segment_start - open - 1..i]);
                segment_start = open + 1 + i + 1;
            }
            _ => {}
        }
    }
    let current = &args[segment_start - open - 1..];
    for segment in segments {
        match segment.split_once(':') {
            Some((name, _)) if !name.trim().is_empty() && name.trim().chars().all(|c| c.is_alphanumeric() || c == '_') => named.push(name.trim()),
            _ => unnamed_before += 1,
        }
    }
    Some(Call { name, receiver, receiver_text, named, unnamed_before, current })
}

/// The settings of an `instr` or `mono`, written in its body, and for `mono` instruments the ones that are
/// for them alone. Each has what it does and the values it takes.
const OPTIONS: &[(&str, bool, &str, &[&str])] = &[
    ("max", false, "The most notes of this instrument sounding at once; the oldest end first", &[]),
    ("steal", false, "Which note ends when `max` is reached", &["oldest", "quietest"]),
    ("same_note", false, "A new note releases the sounding note of the same pitch", &["release"]),
    ("choke", false, "A new note ends the notes in this group, whichever instrument they are of", &[]),
    ("tail", false, "How long a voice keeps sounding after its release, for example `2sec`", &[]),
    ("pitch", false, "The `hz` parameter that notes set, `freq` unless renamed", &[]),
    ("quantize", false, "When a redefinition lands", &["now", "beat", "bar", "cycle"]),
    ("legato", true, "A note that starts over a held one takes over its voice instead of attacking again", &["true", "false"]),
    ("priority", true, "Which held note a legato voice sounds (needs `legato = true`)", &["last", "lowest", "highest"]),
    ("glide", true, "How long a change of pitch takes to slide, for example `80ms`", &[]),
];

/// Where the cursor is in the body of an `instr` or `mono`, directly in it rather than in a block inside:
/// whether it is a `mono`, the body so far, and the line being written.
fn in_instr_body(before: &str) -> Option<(bool, &str, &str)> {
    let mut offset = 0;
    let mut head = None;
    for line in before.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with("instr ") || t.starts_with("mono ") {
            head = Some((offset, t.starts_with("mono ")));
        }
        offset += line.len();
    }
    let (start, mono) = head?;
    let body = &before[start..];
    let depth = body.matches('{').count() as i64 - body.matches('}').count() as i64;
    let line = body.rsplit('\n').next().unwrap_or("").trim_start();
    (depth == 1 && body.contains('{')).then_some((mono, body, line))
}

/// The values of the setting being written, after `name = `.
fn setting_values(before: &str) -> Option<Vec<Value>> {
    let (mono, _, line) = in_instr_body(before)?;
    let (name, value) = line.split_once('=')?;
    let (name, value) = (name.trim(), value.trim());
    if !value.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let (_, _, _, values) = OPTIONS.iter().find(|o| o.0 == name && (mono || !o.1))?;
    (!values.is_empty()).then(|| values.iter().map(|v| item(v, ENUM_MEMBER, name, "")).collect())
}

/// The settings not yet written, at the start of a line in the body.
fn setting_names(before: &str) -> Vec<Value> {
    let Some((mono, body, line)) = in_instr_body(before) else { return Vec::new() };
    if !line.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Vec::new();
    }
    let given = |name: &str| body.lines().any(|l| l.trim_start().strip_prefix(name).is_some_and(|r| r.trim_start().starts_with('=')));
    OPTIONS.iter()
        .filter(|(name, mono_only, ..)| (mono || !mono_only) && !given(name))
        .map(|(name, _, doc, _)| item(&format!("{name} ="), PROPERTY, "", doc))
        .collect()
}

/// Just the given names, for after `alias::`.
pub fn symbol_items(symbols: &[Symbol], user_docs: &dyn Fn(&Symbol) -> String) -> Vec<Value> {
    symbols.iter().map(|s| item(&s.name, symbol_kind(s.kind), &s.detail, &user_docs(s))).collect()
}

/// What a `.` is after, as far as the text before it says.
#[derive(Debug, PartialEq)]
enum Receiver {
    /// A pattern, clip, grid or node: `riff.`, `[c4 e4].`, `it.`, `bass.`
    Pattern,
    /// A part (a clip or node) at the start of a statement: `bass.` gives a command, or a change to what it plays.
    Part,
    /// A scene at the start of a statement: `verse.start`
    SceneCommand,
    /// A scene: `verse.`
    Scene,
    /// A signal, or anything else.
    Signal,
}

/// Find the start of the chain that ends at the end of `text` (`riff.fast(2).thin(10%)`),
/// by walking back over calls, brackets and `.name` parts, and say what it begins with.
fn receiver_and_root<'a>(text: &'a str, symbols: &[Symbol]) -> (Receiver, Option<&'a str>) {
    let mut rest = text.trim_end();
    let mut chained = false;
    loop {
        // a call's arguments or a pattern's brackets
        let mut literal = false;
        if rest.ends_with([')', ']']) {
            literal = rest.ends_with(']');
            let mut depth = 0usize;
            let mut open = None;
            for (i, c) in rest.char_indices().rev() {
                match c {
                    ')' | ']' => depth += 1,
                    '(' | '[' => {
                        depth -= 1;
                        if depth == 0 {
                            open = Some(i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(open) = open else { return (Receiver::Signal, None) };
            rest = rest[..open].trim_end();
        }
        let name_start = rest.char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || *c == '_').last().map_or(rest.len(), |(i, _)| i);
        let name = &rest[name_start..];
        let before = rest[..name_start].trim_end();
        if name.is_empty() {
            // `[c4 e4].` has no name in front of its brackets
            return (if literal { Receiver::Pattern } else { Receiver::Signal }, None);
        }
        if before.ends_with('.') && !before.ends_with("..") {
            rest = before[..before.len() - 1].trim_end();
            chained = true;
            continue;
        }
        if literal {
            // `grid[...]` and `name[...]` are not a bare pattern
            return (Receiver::Signal, None);
        }
        if name == "it" {
            return (Receiver::Pattern, None);
        }
        // at the start of a statement, `bass.` is a command to give; inside an expression it is a value
        let line = text[..name_start].rsplit('\n').next().unwrap_or_default().trim();
        let command = !chained && (line.is_empty() || line.starts_with("at "));
        let kind = match symbols.iter().find(|s| s.name == name).map(|s| s.kind) {
            Some(SymbolKind::Clip | SymbolKind::Part) if command => Receiver::Part,
            Some(SymbolKind::Pattern | SymbolKind::Clip | SymbolKind::Part) => Receiver::Pattern,
            Some(SymbolKind::Scene) if command => Receiver::SceneCommand,
            Some(SymbolKind::Scene) => Receiver::Scene,
            _ => Receiver::Signal,
        };
        return (kind, Some(name));
    }
}

fn receiver(text: &str, symbols: &[Symbol]) -> Receiver {
    receiver_and_root(text, symbols).0
}

pub fn items(before: &str, symbols: &[Symbol], user_docs: &dyn Fn(&Symbol) -> String) -> Vec<Value> {
    let ops = builtin_ops();
    let op_item = |op: &OpInfo| item(&op.name, FUNCTION, &op.signature(), &op.doc);
    let symbol_item = |s: &Symbol| item(&s.name, symbol_kind(s.kind), &s.detail, &user_docs(s));

    // a number just typed: offer its unit
    let word_start = before.char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || *c == '_').last().map_or(before.len(), |(i, _)| i);
    let word = &before[word_start..];
    if word.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return UNITS.iter().map(|(u, d)| item(u, UNIT, d, "")).collect();
    }

    // after a dot: methods
    if before[..word_start].trim_end().ends_with('.') && !before[..word_start].trim_end().ends_with("..") {
        let dot = before[..word_start].trim_end();
        if let Some(root) = receiver_and_root(&dot[..dot.len() - 1], symbols).1
            && let Some(processor) = symbols.iter().find(|s| s.name == root && s.kind == SymbolKind::Processor)
        {
            return processor.params.iter().map(|p| item(&p.name, PROPERTY, &p.label, "Processor automation parameter.")).collect();
        }
        match receiver(&dot[..dot.len() - 1], symbols) {
            Receiver::Pattern => return pattern_ops().iter().map(op_item).collect(),
            Receiver::Part => {
                let mut out = vec![
                    item("start", FUNCTION, "start()", "Play this clip, from its beginning, on the next boundary."),
                    item("stop", FUNCTION, "stop()", "Stop it."),
                    item("mute", FUNCTION, "mute()", "Silence it without stopping it."),
                    item("unmute", FUNCTION, "unmute()", "Let it be heard again."),
                    item("solo", FUNCTION, "solo()", "Hear only this and the other soloed parts."),
                    item("unsolo", FUNCTION, "unsolo()", "Take it out of the solo."),
                    item("undo", FUNCTION, "undo()", "Take the last change made to it off again."),
                    item("clear", FUNCTION, "clear()", "Take every change made to it off, back to what it was started with."),
                ];
                out.extend(pattern_ops().iter().map(op_item));
                return out;
            }
            Receiver::SceneCommand => {
                return vec![
                    item("start", FUNCTION, "start()", "Play every clip in this scene and the.stop rest."),
                    item("stop", FUNCTION, "stop()", "Stop every clip in this scene."),
                ];
            }
            Receiver::Scene => return pattern_ops().iter().filter(|o| o.name == "over").map(op_item).collect(),
            Receiver::Signal => {
                // `drums.`: offer the kit's members when known (from `.slices(n)`).
                if let Some(root) = receiver_and_root(&dot[..dot.len() - 1], symbols).1
                    && let Some(kit) = symbols.iter().find(|s| s.name == root && s.kind == SymbolKind::Kit)
                    && !kit.params.is_empty()
                {
                    return kit
                        .params
                        .iter()
                        .map(|p| item(&p.name, ENUM_MEMBER, &kit.detail, "a sample in this kit"))
                        .collect();
                }
            }
        }
        let mut out: Vec<Value> = ops.iter().filter(|o| o.positional >= 1 && o.params.first().is_some_and(|p| p.ty != "wave")).map(op_item).collect();
        out.extend(symbols.iter().filter(|s| s.kind == SymbolKind::Function).map(symbol_item));
        return out;
    }

    if let Some(values) = setting_values(before) {
        return values;
    }
    let mut out = setting_names(before);
    if let Some(call) = enclosing_call(before) {
        let consumed = usize::from(call.receiver) + call.unnamed_before;
        let in_value = call.current.contains(':');
        let on_pattern = call.receiver && receiver(call.receiver_text, symbols) != Receiver::Signal;
        let patterns = pattern_ops();
        // `bass.set(`: the lanes this part has, from its instrument
        let root = if call.receiver { receiver_and_root(call.receiver_text, symbols).1 } else { None };
        let lanes = root.filter(|_| call.name == "set").and_then(|r| symbols.iter().find(|s| s.name == r && s.kind == SymbolKind::Clip));
        let params: Vec<(String, &str, usize)> = if let Some(clip) = lanes {
            clip.params.iter().map(|p| (p.name.clone(), "", 1)).collect()
        } else if let Some(op) = patterns.iter().find(|o| on_pattern && o.name == call.name) {
            // a pattern operation lists only the arguments after the receiver
            op.params.iter().enumerate().map(|(i, p)| (p.name.clone(), p.ty, i + 1)).collect()
        } else if let Some(op) = ops.iter().find(|o| o.name == call.name) {
            op.params.iter().enumerate().map(|(i, p)| (p.name.clone(), p.ty, i)).collect()
        } else if let Some(s) = symbols.iter().find(|s| s.name == call.name && matches!(s.kind, SymbolKind::Function | SymbolKind::Instrument)) {
            s.params.iter().enumerate().map(|(i, p)| (p.name.clone(), "", i)).collect()
        } else {
            Vec::new()
        };
        if !in_value {
            for (name, ty, index) in &params {
                if *index >= consumed && !call.named.contains(&name.as_str()) {
                    out.push(item(&format!("{name}:"), PROPERTY, ty, ""));
                }
            }
            if let Some(op) = ops.iter().find(|o| o.name == call.name)
                && op.params.get(consumed).is_some_and(|p| p.ty == "wave")
                && call.current.trim().chars().all(|c| c.is_alphanumeric() || c == '_')
            {
                out.extend(waves().map(|w| item(w, ENUM_MEMBER, "waveform", "")));
            }
            if matches!(call.name, "every" | "layer") {
                out.push(item("it", VARIABLE, "the pattern being changed", "Write `it.reverse`, `it.transpose(+7st)`..."));
            }
        } else if let Some((name, _)) = call.current.split_once(':')
            && ops.iter().find(|o| o.name == call.name).and_then(|o| o.params.iter().find(|p| p.name == name.trim())).is_some_and(|p| p.ty == "wave")
        {
            out.extend(waves().map(|w| item(w, ENUM_MEMBER, "waveform", "")));
        }
    }

    out.extend(symbols.iter().map(symbol_item));
    out.extend(ops.iter().map(op_item));
    out.extend(RESERVED_WORDS.iter().map(|k| item(k, KEYWORD, "", "")));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(before: &str) -> Vec<String> {
        let symbols = niminal_compose::symbols("fn wobble(x, rate: hz = 2hz) { x }\ninstr tone(freq: hz) { freq }").unwrap();
        items(before, &symbols, &|_| String::new()).iter().map(|i| i["label"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn a_number_offers_units() {
        assert_eq!(labels("lpf(cutoff: 800")[0], "hz");
        assert!(!labels("lpf(cutoff: 800").contains(&"lpf".to_string()));
    }

    #[test]
    fn a_dot_offers_methods_including_user_opcodes() {
        let l = labels("osc(saw, 440hz).");
        assert!(l.contains(&"lpf".to_string()) && l.contains(&"wobble".to_string()));
        assert!(!l.contains(&"instr".to_string()) && !l.contains(&"osc".to_string()));
    }

    #[test]
    fn a_call_offers_the_arguments_not_yet_given() {
        let l = labels("osc(saw, 440hz).lpf(");
        assert_eq!(&l[..2], ["cutoff:", "res:"]);
        let l = labels("osc(saw, 440hz).lpf(cutoff: 1khz, ");
        assert_eq!(l[0], "res:");
        assert!(!l.contains(&"cutoff:".to_string()));
        assert_eq!(&labels("tone(")[..1], ["freq:"]);
        let l = labels("adsr(");
        assert!(l.contains(&"attack:".to_string()) && l.contains(&"release:".to_string()));
        assert!(labels("").contains(&"adsr".to_string()) && labels("").contains(&"perc".to_string()));
    }

    #[test]
    fn osc_offers_waveforms_in_its_first_place() {
        let l = labels("osc(");
        assert!(l.contains(&"saw".to_string()) && l.contains(&"tri".to_string()));
        assert!(!labels("osc(saw, ").contains(&"saw".to_string()));
    }

    #[test]
    fn elsewhere_offers_names_and_keywords() {
        let l = labels("");
        assert!(l.contains(&"tone".to_string()) && l.contains(&"part".to_string()) && l.contains(&"lpf".to_string()));
    }

    #[test]
    fn a_processor_offers_its_automation_parameters() {
        let labels = labels_in("processor filter(cut: hz = 2khz) { out = it.lpf(cutoff: cut) }", "clip c { filter.");
        assert_eq!(labels, ["cut"]);
    }

    fn labels_in(code: &str, before: &str) -> Vec<String> {
        let symbols = niminal_compose::symbols(code).unwrap();
        items(before, &symbols, &|_| String::new()).iter().map(|i| i["label"].as_str().unwrap().to_string()).collect()
    }

    const SONG: &str = "fn wobble(x, rate: hz = 2hz) { x }\nriff = [c4 e4]\nclip c { notes: [c4] }\nscene verse { lead: riff }\ninstr pluck(freq: hz) { freq }\npart bass { instrument = pluck }\n";

    #[test]
    fn a_dot_after_a_pattern_offers_pattern_operations_only() {
        for before in ["riff.", "x = c.", "x = bass.", "bass.reverse.", "[c4 e4].", "riff.fast(2).", "riff.transpose(+5st).th", "riff.every(2, it.", "x = riff.every(2, "] {
            let l = labels_in(SONG, before);
            if before.ends_with("(2, ") {
                assert!(l.contains(&"it".to_string()), "{before}");
                continue;
            }
            assert!(l.contains(&"every".to_string()) && l.contains(&"transpose".to_string()), "{before}: {l:?}");
            assert!(!l.contains(&"abs".to_string()) && !l.contains(&"lpf".to_string()) && !l.contains(&"wobble".to_string()), "{before}");
        }
    }

    #[test]
    fn a_dot_at_the_start_of_a_statement_offers_commands_for_a_part_or_scene() {
        let l = labels_in(SONG, "bass.");
        assert_eq!(&l[..3], ["start", "stop", "mute"]);
        assert!(l.contains(&"undo".to_string()) && l.contains(&"every".to_string()));
        assert!(labels_in(SONG, "at +2bars c.").contains(&"mute".to_string()));
        assert_eq!(labels_in(SONG, "verse."), ["start", "stop"]);
        // inside an expression the same names are values
        assert!(!labels_in(SONG, "x = c.").contains(&"mute".to_string()));
        assert!(!labels_in(SONG, "bass.reverse.").contains(&"mute".to_string()));
    }

    #[test]
    fn a_dot_after_a_scene_offers_over_and_after_a_signal_still_offers_opcodes() {
        assert_eq!(labels_in(SONG, "x = verse."), ["over"]);
        let l = labels_in(SONG, "osc(saw, 440hz).");
        assert!(l.contains(&"lpf".to_string()) && l.contains(&"wobble".to_string()) && !l.contains(&"every".to_string()));
        assert!(!labels_in(SONG, "pluck(freq: 440hz).").contains(&"every".to_string()));
    }

    #[test]
    fn a_pattern_operation_offers_its_own_arguments() {
        assert_eq!(&labels_in(SONG, "riff.euclid(")[..3], ["hits:", "steps:", "rotation:"]);
        assert_eq!(&labels_in(SONG, "riff.euclid(3, ")[..2], ["steps:", "rotation:"]);
        assert_eq!(labels_in(SONG, "riff.every(2, ")[0], "change:");
    }

    #[test]
    fn set_offers_the_lanes_of_the_parts_instrument() {
        let code = "instr pluck(freq: hz, bright: 0..1 = 0.5) { freq }\nclip bass {\n  instrument: pluck(bright: 0.2)\n  notes: [c2]\n}\n";
        let l = labels_in(code, "bass.set(");
        assert_eq!(&l[..4], ["bright:", "dur:", "nudge:", "gain:"], "{l:?}");
        assert_eq!(&labels_in(code, "bass.set(bright: 0.9, ")[..3], ["dur:", "nudge:", "gain:"]);
        assert!(labels_in(code, "bass.").contains(&"set".to_string()));
    }

    #[test]
    fn an_instrument_body_offers_its_settings_and_mono_ones_only_for_mono() {
        let l = labels_in("", "instr pad(freq: hz) {\n  ");
        assert!(l.contains(&"max =".to_string()) && l.contains(&"choke =".to_string()) && !l.contains(&"legato =".to_string()), "{l:?}");
        let l = labels_in("", "mono bass(freq: hz) {\n  max = 4\n  ma");
        assert!(l.contains(&"legato =".to_string()) && l.contains(&"glide =".to_string()));
        let l = labels_in("", "mono bass(freq: hz) {\n  legato = true\n  ");
        assert!(!l.contains(&"legato =".to_string()));
        assert_eq!(labels_in("", "mono bass(freq: hz) {\n  priority = "), ["last", "lowest", "highest"]);
        // not inside a nested block, or outside an instrument
        assert!(!labels_in("", "instr pad(freq: hz) {\n  if freq > 1hz {\n  ").contains(&"max =".to_string()));
        assert!(!labels_in("", "").contains(&"max =".to_string()));
    }
}
