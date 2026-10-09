//! Pieces of the live commands that don't need a terminal: turning problems
//! into text, choosing a layout, and replaying a log to audio.

use niminal_daemon::{LogEntry, Problem, QuantizeDefaults, Session};
use niminal_lang::Layout;
use serde_json::Value;

use crate::render::SAMPLE_RATE;

/// The layout with this many channels.
pub fn layout_for(channels: u16) -> Result<Layout, String> {
    match channels {
        1 => Ok(Layout::Mono),
        2 => Ok(Layout::Stereo),
        4 => Ok(Layout::Quad),
        6 => Ok(Layout::Surround51),
        8 => Ok(Layout::Surround71),
        n => Err(format!("error: there is no standard layout with {n} channels; use 1, 2, 4, 6 or 8")),
    }
}

/// A problem in `source` as text, in the same style as compile errors.
pub fn render_problem(message: &str, help: Option<&str>, line: Option<usize>, column: Option<usize>, source: &str, name: &str) -> String {
    let mut out = format!("error: {message}\n");
    if let (Some(line), Some(column)) = (line, column) {
        let text = source.lines().nth(line - 1).unwrap_or("");
        let gutter = line.to_string();
        let pad = " ".repeat(gutter.len());
        out.push_str(&format!(
            "{pad}--> {name}:{line}:{column}\n{pad} |\n{gutter} | {text}\n{pad} | {}^\n",
            " ".repeat(column - 1)
        ));
    }
    if let Some(help) = help {
        out.push_str(&format!("  = help: {help}\n"));
    }
    out
}

pub fn render_problems(problems: &[Problem], source: &str, name: &str) -> String {
    problems
        .iter()
        .map(|p| {
            let mut text = render_problem(&p.message, p.help.as_deref(), p.line, p.column, source, name);
            if let Some(key) = &p.in_definition {
                text.push_str(&format!("  = note: this is in the earlier definition `{key}`\n"));
            }
            text
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The problems in an error reply from the daemon, as text.
pub fn render_error(error: &Value, source: &str, name: &str) -> String {
    let problems = error["data"]["problems"].as_array();
    match problems {
        Some(list) if !list.is_empty() => list
            .iter()
            .map(|p| {
                let get = |k: &str| p[k].as_u64().map(|n| n as usize);
                let mut text = render_problem(
                    p["message"].as_str().unwrap_or("error"),
                    p["help"].as_str(),
                    get("line"),
                    get("column"),
                    source,
                    name,
                );
                if let Some(key) = p["in_definition"].as_str() {
                    text.push_str(&format!("  = note: this is in the earlier definition `{key}`\n"));
                }
                text
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => format!("error: {}\n", error["message"].as_str().unwrap_or("the daemon refused that")),
    }
}

/// Play a recorded session back to audio: one buffer per channel, with the
/// limiter's delay removed and trailing silence trimmed.
pub fn replay(log: &[LogEntry], channels: u16, sample_rate: u32, limiter: bool) -> Result<Vec<Vec<f32>>, String> {
    const TAIL_SECONDS: u64 = 30;
    const QUIET: f32 = 3.162_277_7e-5;

    let layout = layout_for(channels)?;
    let last = log.iter().map(|e| e.sample).max().unwrap_or(0);
    let frames = last + TAIL_SECONDS * u64::from(sample_rate);
    let mut audio = Session::replay(sample_rate as f32, layout, QuantizeDefaults::default(), log, frames, limiter);

    if limiter {
        let latency = Session::new(sample_rate as f32, layout).latency();
        for c in &mut audio {
            c.drain(..latency.min(c.len()));
        }
    }
    let loud_until = (0..audio[0].len()).rev().find(|&i| audio.iter().any(|c| c[i].abs() >= QUIET)).map_or(0, |i| i + 1);
    audio.iter_mut().for_each(|c| c.truncate(loud_until));
    Ok(audio)
}

pub const DEFAULT_SAMPLE_RATE: u32 = SAMPLE_RATE;
