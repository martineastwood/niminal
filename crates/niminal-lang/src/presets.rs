//! The ready-made instruments and effects that come with niminal, as one catalog for every program that
//! offers them: `niminal presets`, the tracker's browser, the editor's completions. They are written in the
//! shelf files under `std/` (`std/drums`, `std/fx`, ...), each preset standing alone with a blank line
//! between it and the next, the `///` comment above it as its description, and a `// folder: drums/kicks`
//! comment putting the presets after it in that folder. In code a preset is taken by name,
//! `use std/drums.{kick_808}`; the tracker and editor copy its `source` into a project instead.

use std::sync::OnceLock;

use crate::ide::symbols;
use crate::modules::embedded;

/// The functions from the language itself that presets are settings of. A copy of a preset carries one, so
/// that it stands alone and every control can be edited.
const VOICES: [&str; 1] = ["drum"];

/// The modules made of presets. A plain `use` of one is an error, because it would add every preset.
pub const SHELVES: [&str; 3] = ["std/instruments", "std/drums", "std/fx"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetKind {
    Instrument,
    Effect,
}

impl PresetKind {
    /// The keyword that defines one: `instr` or `fx`.
    pub fn keyword(self) -> &'static str {
        match self {
            PresetKind::Instrument => "instr",
            PresetKind::Effect => "fx",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    /// The name in code: `kick_808`.
    pub id: String,
    /// The name to show: `Kick 808`.
    pub name: String,
    pub kind: PresetKind,
    /// Where it sits in a browser, like `drums/kicks`.
    pub folder: String,
    pub description: String,
    /// The shelf it is taken from: `std/drums`.
    pub shelf: &'static str,
    /// The definition, ready to paste into a file.
    pub source: String,
    /// The functions it calls that are defined in its shelf (the `drum` voice of a drum preset), to paste
    /// along with it.
    pub helpers: String,
    /// Its parameters as written, like `freq: hz = 220hz`.
    pub params: Vec<String>,
}

/// Every preset, shelf by shelf, in the order the files give them.
pub fn all() -> &'static [Preset] {
    static ALL: OnceLock<Vec<Preset>> = OnceLock::new();
    ALL.get_or_init(|| SHELVES.iter().flat_map(|s| parse_shelf(s)).collect())
}

/// The preset called `id`.
pub fn find(id: &str) -> Option<&'static Preset> {
    all().iter().find(|p| p.id == id)
}

/// The presets of one shelf.
pub fn of_shelf(shelf: &str) -> impl Iterator<Item = &'static Preset> + '_ {
    all().iter().filter(move |p| p.shelf == shelf)
}

fn parse_shelf(shelf: &'static str) -> Vec<Preset> {
    let text = embedded(shelf).expect("a shelf is a module that comes with niminal");
    let blocks: Vec<&str> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .collect();
    let defines = |block: &str, keyword: &str| -> Option<String> {
        block.lines().find_map(|line| {
            let rest = line.strip_prefix(keyword)?.strip_prefix(' ')?;
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (!name.is_empty()).then_some(name)
        })
    };
    let helpers: Vec<(String, &str)> = blocks
        .iter()
        .filter_map(|b| defines(b, "fn").map(|name| (name, *b)))
        .collect();
    let mut folder = String::new();
    let mut out = Vec::new();
    for block in blocks {
        let kind = [PresetKind::Instrument, PresetKind::Effect]
            .into_iter()
            .find_map(|k| defines(block, k.keyword()).map(|id| (k, id)));
        let Some((kind, id)) = kind else {
            if let Some(f) = block
                .lines()
                .find_map(|l| l.strip_prefix("// folder:"))
            {
                folder = f.trim().to_string();
            }
            continue;
        };
        let description = block
            .lines()
            .take_while(|l| l.starts_with("//"))
            .filter_map(|l| l.strip_prefix("///"))
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ");
        let source = format!("{block}\n");
        let helpers = helpers
            .iter()
            .filter(|(name, _)| block.contains(&format!("{name}(")))
            .map(|(_, b)| format!("{b}\n"))
            .chain(
                VOICES
                    .iter()
                    .filter(|name| block.contains(&format!("{name}(")))
                    .filter_map(|name| crate::prelude::source_of(name)),
            )
            .collect::<Vec<_>>()
            .join("\n");
        let params = symbols(&source)
            .ok()
            .and_then(|syms| syms.into_iter().find(|s| s.name == id))
            .map(|s| s.params.into_iter().map(|p| p.label).collect())
            .unwrap_or_default();
        let name = {
            let spaced = id.replace('_', " ");
            let mut chars = spaced.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        };
        out.push(Preset {
            id,
            name,
            kind,
            folder: folder.clone(),
            description,
            shelf,
            source,
            helpers,
            params,
        });
    }
    out
}

/// The catalog as JSON, for programs that read it from a file: `std/presets.json`.
pub fn json() -> String {
    fn string(s: &str) -> String {
        let mut out = String::from("\"");
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }
    let items: Vec<String> = all()
        .iter()
        .map(|p| {
            format!(
                "  {{\"id\":{},\"name\":{},\"kind\":{},\"folder\":{},\"description\":{},\"shelf\":{},\"source\":{},\"helpers\":{},\"params\":[{}]}}",
                string(&p.id),
                string(&p.name),
                string(p.kind.keyword()),
                string(&p.folder),
                string(&p.description),
                string(p.shelf),
                string(&p.source),
                string(&p.helpers),
                p.params.iter().map(|l| string(l)).collect::<Vec<_>>().join(",")
            )
        })
        .collect();
    format!("[\n{}\n]\n", items.join(",\n"))
}
