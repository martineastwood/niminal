//! The C interface: what a host calls on the loaded module. One render's result is kept until the next.

use std::sync::Mutex;

use crate::{Failure, Rendered, render_piece};

#[derive(Default)]
struct State {
    rendered: Option<Rendered>,
    /// JSON: `{"diagnostics": [{start, end, message, help}], "message": "...", "warnings": ["..."]}`.
    report: String,
    panic: String,
}

static STATE: Mutex<State> = Mutex::new(State {
    rendered: None,
    report: String::new(),
    panic: String::new(),
});

fn lock_state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Call once after loading: a panic's message is kept for [`niminal_panic_message`], since the target has
/// nowhere to print it.
#[unsafe(no_mangle)]
pub extern "C" fn niminal_init() {
    std::panic::set_hook(Box::new(|info| lock_state().panic = info.to_string()));
}

/// Room for `n` bytes of the host's writing, which it gives back with [`niminal_free`].
#[unsafe(no_mangle)]
pub extern "C" fn niminal_alloc(n: usize) -> *mut u8 {
    let mut bytes = Vec::<u8>::with_capacity(n.max(1));
    let pointer = bytes.as_mut_ptr();
    std::mem::forget(bytes);
    pointer
}

/// # Safety
/// `pointer` came from [`niminal_alloc`] with the same `n`, and is not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn niminal_free(pointer: *mut u8, n: usize) {
    // SAFETY: the caller's contract: this is the allocation `niminal_alloc` made, of capacity `n.max(1)`.
    drop(unsafe { Vec::from_raw_parts(pointer, 0, n.max(1)) });
}

/// Render the UTF-8 source at `source` (`length` bytes) for `bars` bars. Returns 0 for audio (read it with the
/// functions below), 1 if the source has mistakes or cannot be rendered (read [`niminal_report`]).
///
/// # Safety
/// `source` points to `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn niminal_render(source: *const u8, length: usize, bars: f64) -> i32 {
    // SAFETY: the caller's contract.
    let bytes = unsafe { std::slice::from_raw_parts(source, length) };
    let mut state = lock_state();
    state.rendered = None;
    let Ok(text) = std::str::from_utf8(bytes) else {
        state.report = report(&[], "the source is not valid UTF-8", &[]);
        return 1;
    };
    // the render runs without the state lock, which a panic hook needs
    drop(state);
    let result = render_piece(text, bars);
    let mut state = lock_state();
    match result {
        Ok(rendered) => {
            state.report = report(&[], "", &rendered.warnings);
            state.rendered = Some(rendered);
            0
        }
        Err(Failure::Source(diagnostics)) => {
            state.report = report(&diagnostics, "", &[]);
            1
        }
        Err(Failure::Other(message)) => {
            state.report = report(&[], &message, &[]);
            1
        }
    }
}

fn report(diagnostics: &[niminal_lang::Diagnostic], message: &str, warnings: &[String]) -> String {
    serde_json::json!({
        "diagnostics": diagnostics.iter().map(|d| serde_json::json!({
            "start": d.span.start,
            "end": d.span.end,
            "message": d.message,
            "help": d.help,
        })).collect::<Vec<_>>(),
        "message": message,
        "warnings": warnings,
    })
    .to_string()
}

/// The last report as JSON: pointer to its bytes (see [`niminal_report_length`]).
#[unsafe(no_mangle)]
pub extern "C" fn niminal_report() -> *const u8 {
    lock_state().report.as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn niminal_report_length() -> usize {
    lock_state().report.len()
}

/// The channels of the last successful render (0 if none).
#[unsafe(no_mangle)]
pub extern "C" fn niminal_channels() -> usize {
    lock_state().rendered.as_ref().map_or(0, |r| r.channels.len())
}

/// Samples per channel.
#[unsafe(no_mangle)]
pub extern "C" fn niminal_frames() -> usize {
    lock_state()
        .rendered
        .as_ref()
        .and_then(|r| r.channels.first())
        .map_or(0, Vec::len)
}

#[unsafe(no_mangle)]
pub extern "C" fn niminal_sample_rate() -> u32 {
    lock_state().rendered.as_ref().map_or(0, |r| r.sample_rate)
}

/// A pointer to `channel`'s samples (`f32`, [`niminal_frames`] of them), valid until the next render.
#[unsafe(no_mangle)]
pub extern "C" fn niminal_channel(channel: usize) -> *const f32 {
    lock_state()
        .rendered
        .as_ref()
        .and_then(|r| r.channels.get(channel))
        .map_or(std::ptr::null(), |c| c.as_ptr())
}

/// The message of the last panic, if the host caught a trap: pointer and length.
#[unsafe(no_mangle)]
pub extern "C" fn niminal_panic_message() -> *const u8 {
    lock_state().panic.as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn niminal_panic_length() -> usize {
    lock_state().panic.len()
}
