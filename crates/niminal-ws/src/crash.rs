//! Caught audio panics must return to silence before diagnostics touch a terminal or disk.
use std::cell::Cell;
use std::collections::VecDeque;
use std::fmt::{self, Write};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

thread_local! {
    static DEFER: Cell<bool> = const { Cell::new(false) };
}

/// Report panics in `run` asynchronously. Use around a callback that catches and recovers from them.
/// Audio-worker panics are deferred too, before they are propagated back to the callback.
pub fn defer_panic_reports<R>(run: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            DEFER.set(self.0);
        }
    }
    let _restore = Restore(DEFER.replace(true));
    run()
}

const QUEUED: usize = 16;
struct Report {
    bytes: [u8; 2048],
    len: usize,
}
impl Report {
    fn new(args: fmt::Arguments<'_>) -> Self {
        let mut report = Self {
            bytes: [0; 2048],
            len: 0,
        };
        let _ = report.write_fmt(args);
        report
    }
    fn text(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).expect("reports contain UTF-8")
    }
}
impl Write for Report {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let mut end = text.len().min(self.bytes.len() - self.len);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.bytes[self.len..self.len + end].copy_from_slice(&text.as_bytes()[..end]);
        self.len += end;
        Ok(())
    }
}

struct Reports {
    pending: Mutex<VecDeque<Report>>,
    dropped: AtomicUsize,
}
impl Reports {
    fn new() -> Self {
        Self {
            pending: Mutex::new(VecDeque::with_capacity(QUEUED)),
            dropped: AtomicUsize::new(0),
        }
    }
    fn submit(&self, report: Report) {
        // Never wait for another panic or for the reporter, even on this exceptional path.
        if let Ok(mut pending) = self.pending.try_lock()
            && pending.len() < QUEUED
        {
            pending.push_back(report);
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn write_report(log: Option<&PathBuf>, app: &str, version: &str, report: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    {
        use std::io::Write;
        let _ = writeln!(std::io::stderr().lock(), "{app} {version} {report}");
    }
    if let Some(dir) = log {
        let _ = std::fs::create_dir_all(dir);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("crash.log"))
        {
            use std::io::Write;
            let _ = writeln!(file, "[{now}] {app} {version} {report}");
        }
    }
}

/// Log panics to `~/.niminal/crash.log`. Caught audio panics keep their message, location and thread,
/// but use a bounded, preallocated report instead of capturing a backtrace or doing I/O in the hook.
/// Other panics retain synchronous reporting so their logs survive process exit.
pub fn install_crash_log(app: &'static str, version: &'static str) {
    let log = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".niminal"));
    install(app, version, log);
}

fn install(app: &'static str, version: &'static str, log: Option<PathBuf>) {
    let reports = Arc::new(Reports::new());
    let worker_reports = reports.clone();
    let worker_log = log.clone();
    let worker = std::thread::Builder::new()
        .name("niminal-crash-log".into())
        .spawn(move || {
            loop {
                let report = worker_reports
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .pop_front();
                if let Some(report) = report {
                    write_report(worker_log.as_ref(), app, version, report.text());
                    continue;
                }
                let dropped = worker_reports.dropped.swap(0, Ordering::Relaxed);
                if dropped > 0 {
                    write_report(
                        worker_log.as_ref(),
                        app,
                        version,
                        &format!("{dropped} additional panic reports dropped"),
                    );
                }
                std::thread::park();
            }
        })
        .expect("start crash reporter");
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("?");
        if DEFER.get() || name.starts_with("niminal-dsp-") || name == "niminal-plan" {
            reports.submit(Report::new(format_args!("panicked in thread `{name}`: {info}")));
            worker.thread().unpark();
        } else {
            previous(info);
            write_report(
                log.as_ref(),
                app,
                version,
                &format!(
                    "panicked in thread `{name}`: {info}\n{}",
                    std::backtrace::Backtrace::force_capture()
                ),
            );
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    // The panic hook is process-wide, including the other test that deliberately unwinds.
    static HOOK_TEST: Mutex<()> = Mutex::new(());

    #[test]
    fn caught_audio_panics_bypass_the_synchronous_hook_and_are_logged_by_the_worker() {
        let _hook = HOOK_TEST.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("niminal-crash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let original = std::panic::take_hook();
        let synchronous = Arc::new(AtomicUsize::new(0));
        let called = synchronous.clone();
        std::panic::set_hook(Box::new(move |_| {
            called.fetch_add(1, Ordering::Relaxed);
        }));
        install("test", "0", Some(dir.clone()));
        let audio_panic =
            std::panic::catch_unwind(|| defer_panic_reports(|| panic!("audio diagnostic")));
        let workers_panicked = ["niminal-plan", "niminal-dsp-test"].into_iter().all(|name| {
            std::thread::Builder::new()
                .name(name.into())
                .spawn(|| std::panic::catch_unwind(|| panic!("caught worker diagnostic")))
                .unwrap()
                .join()
                .unwrap()
                .is_err()
        });
        let audio_calls = synchronous.load(Ordering::Relaxed);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let logged = loop {
            let text = std::fs::read_to_string(dir.join("crash.log")).unwrap_or_default();
            if text.contains("audio diagnostic") || std::time::Instant::now() >= deadline {
                break text;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let fatal_panic = std::panic::catch_unwind(|| panic!("fatal diagnostic"));
        let fatal_calls = synchronous.load(Ordering::Relaxed);
        let fatal_log = std::fs::read_to_string(dir.join("crash.log")).unwrap_or_default();
        std::panic::set_hook(original);
        let _ = std::fs::remove_dir_all(dir);
        assert!(audio_panic.is_err() && workers_panicked && fatal_panic.is_err());
        assert_eq!(audio_calls, 0);
        assert!(logged.contains("audio diagnostic") && logged.contains("crash.rs"));
        assert_eq!(fatal_calls, 1);
        assert!(fatal_log.contains("fatal diagnostic"));
    }

    #[test]
    fn reports_are_bounded_and_do_not_wait_for_the_reporter() {
        let reports = Reports::new();
        let held = reports.pending.lock().unwrap();
        reports.submit(Report::new(format_args!("while the reporter is busy")));
        assert_eq!(reports.dropped.load(Ordering::Relaxed), 1);
        drop(held);
        for _ in 0..QUEUED + 1 {
            reports.submit(Report::new(format_args!("another panic")));
        }
        assert_eq!(reports.pending.lock().unwrap().len(), QUEUED);
        assert_eq!(reports.dropped.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn long_unicode_reports_stay_valid_utf8() {
        let report = Report::new(format_args!("{}", "€".repeat(1000)));
        assert_eq!(report.text(), "€".repeat(682));
    }

    #[test]
    fn reporting_scope_is_restored_after_unwinding() {
        let _hook = HOOK_TEST.lock().unwrap();
        assert!(!DEFER.get());
        let panic = std::panic::catch_unwind(|| {
            defer_panic_reports(|| {
                assert!(DEFER.get());
                panic!("caught by the callback");
            })
        });
        assert!(panic.is_err());
        assert!(!DEFER.get());
    }
}
