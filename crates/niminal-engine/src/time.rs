//! Clocks that work on every target. `std::time::Instant::now()` panics on `wasm32-unknown-unknown`, which has
//! no clock of its own; there the host provides one by importing `env.niminal_now_ms` (a browser answers it
//! with `performance.now()`).

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use std::time::Instant;

/// Microseconds on a wall clock, for stamping when something happened. Not an absolute date on the web.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub fn wall_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub use web::{Instant, wall_micros};

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod web {
    use std::ops::{Add, Sub};
    use std::time::Duration;

    #[link(wasm_import_module = "env")]
    unsafe extern "C" {
        fn niminal_now_ms() -> f64;
    }

    fn now_ms() -> f64 {
        // SAFETY: the host's import takes no arguments and returns a number.
        unsafe { niminal_now_ms() }
    }

    pub fn wall_micros() -> u64 {
        (now_ms() * 1000.0) as u64
    }

    /// A point in time, in milliseconds on the host's clock.
    #[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
    pub struct Instant(f64);

    impl Instant {
        pub fn now() -> Instant {
            Instant(now_ms())
        }

        pub fn elapsed(&self) -> Duration {
            Instant::now() - *self
        }

        pub fn duration_since(&self, earlier: Instant) -> Duration {
            *self - earlier
        }

        pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
            *self - earlier
        }
    }

    impl Sub for Instant {
        type Output = Duration;
        fn sub(self, other: Instant) -> Duration {
            Duration::from_secs_f64(((self.0 - other.0) / 1000.0).max(0.0))
        }
    }

    impl Add<Duration> for Instant {
        type Output = Instant;
        fn add(self, d: Duration) -> Instant {
            Instant(self.0 + d.as_secs_f64() * 1000.0)
        }
    }

    impl Sub<Duration> for Instant {
        type Output = Instant;
        fn sub(self, d: Duration) -> Instant {
            Instant(self.0 - d.as_secs_f64() * 1000.0)
        }
    }
}
