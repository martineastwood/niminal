//! Discover editable presets and export the bundled original recordings.

use std::path::Path;

use niminal_lang::presets::{PresetKind, all};

pub fn presets(name: Option<&str>, kind: Option<&str>) -> Result<(), String> {
    let matches_kind = |k| match kind {
        Some("instrument") => k == PresetKind::Instrument,
        Some("fx") => k == PresetKind::Effect,
        _ => true,
    };
    if let Some(name) = name {
        let preset = all()
            .iter()
            .find(|p| p.id == name && matches_kind(p.kind))
            .ok_or_else(|| format!("no preset `{name}`; run `niminal presets` to list them"))?;
        if !preset.helpers.is_empty() {
            println!("{}", preset.helpers);
        }
        print!("{}", preset.source);
    } else {
        for p in all().iter().filter(|p| matches_kind(p.kind)) {
            println!(
                "{}\t{}\t{}\t{}",
                p.id,
                p.kind.keyword(),
                p.folder,
                p.description
            );
        }
    }
    Ok(())
}

pub fn samples(out: &Path) -> Result<(), String> {
    // A new directory keeps an export from replacing someone's edited patches or recordings.
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::create_dir(out)
        .map_err(|e| format!("{}: {e}; choose a new output directory", out.display()))?;
    for (name, data) in niminal_lang::recordings::FILES {
        let path = out.join(name);
        std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    println!(
        "exported 12 original recordings, source patches and starter.nml to {}",
        out.display()
    );
    Ok(())
}
