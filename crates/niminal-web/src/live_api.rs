//! The C interface to a live session (see [`crate::live`]): start one, run code in it, take audio from it a chunk
//! at a time, ask what is playing. There is one session per module instance.

use std::cell::RefCell;

use crate::live::{Landed, LiveFailure, LiveSession};

#[derive(Default)]
struct State {
    session: Option<LiveSession>,
    /// The last JSON answer: what a start or an evaluation did, or what is playing.
    text: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

fn landed_json(landed: &Landed) -> String {
    serde_json::json!({ "bar": landed.bar, "beat": landed.beat, "changes": landed.changes }).to_string()
}

fn failure_json(failure: &LiveFailure) -> String {
    let diagnostics: Vec<serde_json::Value> = match failure {
        LiveFailure::Source(errors) => errors
            .iter()
            .map(|d| serde_json::json!({ "start": d.span.start, "end": d.span.end, "message": d.message, "help": d.help }))
            .collect(),
        LiveFailure::Snippet(problems) => problems
            .iter()
            .map(|p| {
                serde_json::json!({
                    "start": p.span.map(|s| s.start),
                    "end": p.span.map(|s| s.end),
                    "message": p.message,
                    "help": p.help,
                    "line": p.line,
                    "column": p.column,
                })
            })
            .collect(),
        LiveFailure::Other(_) => Vec::new(),
    };
    let message = match failure {
        LiveFailure::Other(message) => message.as_str(),
        _ => "",
    };
    serde_json::json!({ "diagnostics": diagnostics, "message": message }).to_string()
}

/// The text a call left (see each function): a pointer to its bytes, [`niminal_live_text_length`] of them.
#[unsafe(no_mangle)]
pub extern "C" fn niminal_live_text() -> *const u8 {
    with(|s| s.text.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn niminal_live_text_length() -> usize {
    with(|s| s.text.len())
}

/// Start a session at `sample_rate` for the UTF-8 source at `source` (`length` bytes) and run all of it now,
/// replacing any session there was (a piece that fails to start leaves it alone). Returns 0 (the text is what landed, as JSON) or 1 (the text is the mistakes).
///
/// # Safety
/// `source` points to `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn niminal_live_start(source: *const u8, length: usize, sample_rate: f32) -> i32 {
    // SAFETY: the caller's contract.
    let bytes = unsafe { std::slice::from_raw_parts(source, length) };
    let Ok(text) = std::str::from_utf8(bytes) else {
        with(|s| s.text = failure_json(&LiveFailure::Other("the source is not valid UTF-8".into())));
        return 1;
    };
    // built outside `with`: starting compiles, and a panic must not leave the state borrowed
    let started = LiveSession::start(text, sample_rate);
    with(|s| match started {
        Ok((session, landed)) => {
            s.text = landed_json(&landed);
            s.session = Some(session);
            0
        }
        // a piece that does not start leaves the session that was running as it was
        Err(failure) => {
            s.text = failure_json(&failure);
            1
        }
    })
}

/// Run the UTF-8 source at `source` in the session. The text at `when` (`when_length` bytes, may be empty) says when
/// it lands: `now`, `beat`, `bar`, `next 4bars`...; empty leaves each change to land where it asks. Returns 0 (what
/// lands) or 1 (the mistakes, located in the snippet; a session that is not started is a mistake).
///
/// # Safety
/// `source` points to `length` readable bytes and `when` to `when_length`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn niminal_live_eval(source: *const u8, length: usize, when: *const u8, when_length: usize) -> i32 {
    // SAFETY: the caller's contract.
    let bytes = unsafe { std::slice::from_raw_parts(source, length) };
    let text = String::from_utf8_lossy(bytes).into_owned();
    // SAFETY: the caller's contract.
    let quantize = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(when, when_length) }).into_owned();
    let quantize = (!quantize.is_empty()).then_some(quantize);
    with(|s| {
        let Some(session) = s.session.as_mut() else {
            s.text = failure_json(&LiveFailure::Other("the engine is not running: run the piece first".into()));
            return 1;
        };
        match session.eval(&text, quantize.as_deref()) {
            Ok(landed) => {
                s.text = landed_json(&landed);
                0
            }
            Err(failure) => {
                s.text = failure_json(&failure);
                1
            }
        }
    })
}

/// Mix the next `frames` samples; returns the number of channels (0 with no session). Read each channel with
/// [`niminal_live_channel`].
#[unsafe(no_mangle)]
pub extern "C" fn niminal_live_process(frames: usize) -> usize {
    with(|s| {
        let Some(session) = s.session.as_mut() else { return 0 };
        session.process(frames);
        session.out.len()
    })
}

/// A pointer to `channel` of the last chunk (`f32`), valid until the next [`niminal_live_process`].
#[unsafe(no_mangle)]
pub extern "C" fn niminal_live_channel(channel: usize) -> *const f32 {
    with(|s| {
        s.session
            .as_ref()
            .and_then(|session| session.out.get(channel))
            .map_or(std::ptr::null(), |c| c.as_ptr())
    })
}

/// What is playing, as JSON in the text; returns its length (0 with no session).
#[unsafe(no_mangle)]
pub extern "C" fn niminal_live_status() -> usize {
    with(|s| {
        s.text = s.session.as_ref().map(LiveSession::status).unwrap_or_default();
        s.text.len()
    })
}

/// Stop every sound and clip now, keeping the session and what it has defined.
#[unsafe(no_mangle)]
pub extern "C" fn niminal_live_hush() {
    with(|s| {
        if let Some(session) = s.session.as_mut() {
            session.hush();
        }
    });
}

/// Move a control of the session now. The UTF-8 text at `name` is the control's name, then (optionally) a `|` and
/// the unit its value is in. Returns 0, or 1 with the reason as the text.
///
/// # Safety
/// `name` points to `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn niminal_live_control(name: *const u8, length: usize, value: f64) -> i32 {
    // SAFETY: the caller's contract.
    let bytes = unsafe { std::slice::from_raw_parts(name, length) };
    let text = String::from_utf8_lossy(bytes).into_owned();
    let (name, unit) = match text.split_once('|') {
        Some((name, unit)) if !unit.is_empty() => (name, Some(unit)),
        Some((name, _)) => (name, None),
        None => (text.as_str(), None),
    };
    with(|s| {
        let Some(session) = s.session.as_mut() else {
            s.text = "the engine is not running: run the piece first".into();
            return 1;
        };
        match session.set_control(name, value, unit) {
            Ok(()) => 0,
            Err(message) => {
                s.text = message;
                1
            }
        }
    })
}

/// End the session and free what it held.
#[unsafe(no_mangle)]
pub extern "C" fn niminal_live_stop() {
    with(|s| s.session = None);
}
