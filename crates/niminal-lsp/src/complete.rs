//! What to offer at the cursor.

use niminal_lang::{OpInfo, Symbol, SymbolKind, builtin_ops, pattern_ops, waves};
use serde_json::{Value, json};

const KEYWORDS: &[&str] = &[
    "instr", "opcode", "bus", "sample", "kit", "ctl", "track", "clip", "scene", "arrangement", "config", "tempo", "meter",
    "play", "stop", "mute", "unmute", "solo", "unsolo", "launch", "hush", "panic", "reset", "at", "for", "over", "next", "now",
];
const UNITS: &[(&str, &str)] = &[
    ("hz", "frequency"), ("khz", "frequency, in thousands of hz"), ("db", "gain in decibels"), ("ms", "milliseconds"),
    ("sec", "seconds"), ("beat", "beats, at the tempo"), ("beats", "beats, at the tempo"), ("bar", "bars, at the tempo and meter"),
    ("bars", "bars, at the tempo and meter"), ("deg", "an angle in degrees"), ("st", "semitones"),
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
        SymbolKind::Instrument | SymbolKind::Opcode => FUNCTION,
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

/// Just the given names, for after `alias::`.
pub fn symbol_items(symbols: &[Symbol], user_docs: &dyn Fn(&Symbol) -> String) -> Vec<Value> {
    symbols.iter().map(|s| item(&s.name, symbol_kind(s.kind), &s.detail, &user_docs(s))).collect()
}

/// What a `.` is after, as far as the text before it says.
#[derive(Debug, PartialEq)]
enum Receiver {
    /// A pattern, clip, grid or track: `riff.`, `[c4 e4].`, `it.`, `bass.`
    Pattern,
    /// A track: `bass.` changes what it plays, and can take changes off again.
    Track,
    /// A scene: `verse.`
    Scene,
    /// A signal, or anything else.
    Signal,
}

/// Find the start of the chain that ends at the end of `text` (`riff.fast(2).thin(10%)`),
/// by walking back over calls, brackets and `.name` parts, and say what it begins with.
fn receiver(text: &str, symbols: &[Symbol]) -> Receiver {
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
            let Some(open) = open else { return Receiver::Signal };
            rest = rest[..open].trim_end();
        }
        let name_start = rest.char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || *c == '_').last().map_or(rest.len(), |(i, _)| i);
        let name = &rest[name_start..];
        let before = rest[..name_start].trim_end();
        if name.is_empty() {
            // `[c4 e4].` has no name in front of its brackets
            return if literal { Receiver::Pattern } else { Receiver::Signal };
        }
        if before.ends_with('.') && !before.ends_with("..") {
            rest = before[..before.len() - 1].trim_end();
            chained = true;
            continue;
        }
        if literal {
            // `grid[...]` and `name[...]` are not a bare pattern
            return Receiver::Signal;
        }
        if name == "it" {
            return Receiver::Pattern;
        }
        return match symbols.iter().find(|s| s.name == name).map(|s| s.kind) {
            Some(SymbolKind::Pattern | SymbolKind::Clip) => Receiver::Pattern,
            Some(SymbolKind::Track) if chained => Receiver::Pattern,
            Some(SymbolKind::Track) => Receiver::Track,
            Some(SymbolKind::Scene) => Receiver::Scene,
            _ => Receiver::Signal,
        };
    }
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
        match receiver(&dot[..dot.len() - 1], symbols) {
            Receiver::Pattern => return pattern_ops().iter().map(op_item).collect(),
            Receiver::Track => {
                let mut out = vec![
                    item("undo", FUNCTION, "undo()", "Take the last change made to this track off again."),
                    item("clear", FUNCTION, "clear()", "Take every change made to this track off, back to what it was started with."),
                ];
                out.extend(pattern_ops().iter().map(op_item));
                return out;
            }
            Receiver::Scene => return pattern_ops().iter().filter(|o| o.name == "over").map(op_item).collect(),
            Receiver::Signal => {}
        }
        let mut out: Vec<Value> = ops.iter().filter(|o| o.positional >= 1 && o.params.first().is_some_and(|p| p.ty != "wave")).map(op_item).collect();
        out.extend(symbols.iter().filter(|s| s.kind == SymbolKind::Opcode).map(symbol_item));
        return out;
    }

    let mut out = Vec::new();
    if let Some(call) = enclosing_call(before) {
        let consumed = usize::from(call.receiver) + call.unnamed_before;
        let in_value = call.current.contains(':');
        let on_pattern = call.receiver && receiver(call.receiver_text, symbols) != Receiver::Signal;
        let patterns = pattern_ops();
        let params: Vec<(String, &str, usize)> = if let Some(op) = patterns.iter().find(|o| on_pattern && o.name == call.name) {
            // a pattern operation lists only the arguments after the receiver
            op.params.iter().enumerate().map(|(i, p)| (p.name.clone(), p.ty, i + 1)).collect()
        } else if let Some(op) = ops.iter().find(|o| o.name == call.name) {
            op.params.iter().enumerate().map(|(i, p)| (p.name.clone(), p.ty, i)).collect()
        } else if let Some(s) = symbols.iter().find(|s| s.name == call.name && matches!(s.kind, SymbolKind::Opcode | SymbolKind::Instrument)) {
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
    out.extend(KEYWORDS.iter().map(|k| item(k, KEYWORD, "", "")));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(before: &str) -> Vec<String> {
        let symbols = niminal_lang::symbols("opcode wobble(x, rate: hz = 2hz) { x }\ninstr tone(freq: hz) { freq }").unwrap();
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
        assert!(l.contains(&"tone".to_string()) && l.contains(&"track".to_string()) && l.contains(&"lpf".to_string()));
    }

    fn labels_in(code: &str, before: &str) -> Vec<String> {
        let symbols = niminal_lang::symbols(code).unwrap();
        items(before, &symbols, &|_| String::new()).iter().map(|i| i["label"].as_str().unwrap().to_string()).collect()
    }

    const SONG: &str = "opcode wobble(x, rate: hz = 2hz) { x }\nriff = [c4 e4]\nclip c { notes: [c4] }\nscene verse { lead: riff }\ninstr pluck(freq: hz) { freq }\ntrack bass { instrument = pluck }\n";

    #[test]
    fn a_dot_after_a_pattern_offers_pattern_operations_only() {
        for before in ["riff.", "c.", "bass.", "bass.reverse.", "[c4 e4].", "riff.fast(2).", "riff.transpose(+5st).th", "riff.every(2, it.", "x = riff.every(2, "] {
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
    fn a_dot_after_a_track_also_offers_undo_and_clear() {
        let l = labels_in(SONG, "bass.");
        assert_eq!(&l[..2], ["undo", "clear"]);
        assert!(l.contains(&"every".to_string()));
        assert!(!labels_in(SONG, "riff.").contains(&"undo".to_string()));
    }

    #[test]
    fn a_dot_after_a_scene_offers_over_and_after_a_signal_still_offers_opcodes() {
        assert_eq!(labels_in(SONG, "verse."), ["over"]);
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
}
