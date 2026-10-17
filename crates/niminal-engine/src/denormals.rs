//! Flush-to-zero: while a [`FlushToZero`] guard lives, the CPU treats numbers too small to be normal
//! floats as zero, instead of working them out slowly (many times slower on x86). This is the one
//! switch that covers every opcode, including feedback loops the user writes in the language.
//!
//! Live, offline and replayed audio must all render under the same guard, or their lowest bits would
//! differ; the mixer takes it, so they do. WebAssembly has no such switch, so the ops that feed back on
//! themselves also flush their own state (see [`flush`]).

/// The size below which feedback state is set to zero. Far below hearing (-300 dB), and far above the
/// smallest normal `f32` (about 1e-38), so state settles to exact zeros whatever the hardware does.
const FLUSH_BELOW: f64 = 1e-15;

/// `x`, or zero if it is too small to matter.
#[inline]
pub fn flush(x: f64) -> f64 {
    if x.abs() < FLUSH_BELOW { 0.0 } else { x }
}

/// `x`, or zero if it is too small to matter.
#[inline]
pub fn flush32(x: f32) -> f32 {
    if f64::from(x).abs() < FLUSH_BELOW { 0.0 } else { x }
}

/// Denormals are flushed to zero for as long as this exists, and the floating-point mode is put back
/// when it is dropped. One made while another is alive on the thread only looks at the mode, which is
/// far cheaper than changing it, so a caller can take one for a long stretch of rendering and the
/// mixer's own, for each block inside it, costs next to nothing.
pub struct FlushToZero {
    /// The mode to put back, if this guard is the one that changed it.
    saved: Option<Mode>,
}

impl FlushToZero {
    pub fn new() -> Self {
        FlushToZero { saved: set_mode() }
    }
}

impl Default for FlushToZero {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for FlushToZero {
    fn drop(&mut self) {
        if let Some(saved) = self.saved {
            restore_mode(saved);
        }
    }
}

#[cfg(target_arch = "x86_64")]
type Mode = u32;

/// MXCSR bits: flush results to zero (FTZ) and treat inputs as zero (DAZ).
#[cfg(target_arch = "x86_64")]
const FTZ_DAZ: u32 = (1 << 15) | (1 << 6);

#[cfg(target_arch = "x86_64")]
fn set_mode() -> Option<Mode> {
    let mut saved: u32 = 0;
    // SAFETY: reads the calling thread's MXCSR into `saved`, a local that outlives the instruction.
    unsafe { std::arch::asm!("stmxcsr [{}]", in(reg) &mut saved, options(nostack, preserves_flags)) };
    if saved & FTZ_DAZ == FTZ_DAZ {
        return None;
    }
    let flushing = saved | FTZ_DAZ;
    // SAFETY: loads a MXCSR value made by setting two bits of the one just read, so no exception is unmasked.
    unsafe { std::arch::asm!("ldmxcsr [{}]", in(reg) &flushing, options(nostack, readonly, preserves_flags)) };
    Some(saved)
}

#[cfg(target_arch = "x86_64")]
fn restore_mode(saved: Mode) {
    // SAFETY: loads back the MXCSR value that `set_mode` read.
    unsafe { std::arch::asm!("ldmxcsr [{}]", in(reg) &saved, options(nostack, readonly, preserves_flags)) };
}

#[cfg(target_arch = "aarch64")]
type Mode = u64;

/// FPCR.FZ: flush denormal inputs and results to zero.
#[cfg(target_arch = "aarch64")]
const FZ: u64 = 1 << 24;

#[cfg(target_arch = "aarch64")]
fn set_mode() -> Option<Mode> {
    let saved: u64;
    // SAFETY: reads the calling thread's FPCR.
    unsafe { std::arch::asm!("mrs {}, fpcr", out(reg) saved, options(nostack, preserves_flags)) };
    if saved & FZ != 0 {
        return None;
    }
    // SAFETY: writes back the value just read with one more bit set, which only changes how tiny numbers are treated.
    unsafe { std::arch::asm!("msr fpcr, {}", in(reg) saved | FZ, options(nostack, preserves_flags)) };
    Some(saved)
}

#[cfg(target_arch = "aarch64")]
fn restore_mode(saved: Mode) {
    // SAFETY: writes back the FPCR value that `set_mode` read.
    unsafe { std::arch::asm!("msr fpcr, {}", in(reg) saved, options(nostack, preserves_flags)) };
}

// Anywhere else (WebAssembly, for one) there is no mode to set.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
type Mode = ();

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn set_mode() -> Option<Mode> {
    None
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn restore_mode(_: Mode) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hint::black_box;

    #[test]
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    fn tiny_numbers_are_zero_only_while_the_guard_lives() {
        let tiny = || black_box(f32::MIN_POSITIVE) * black_box(0.5);
        assert!(tiny().is_subnormal(), "the machine does not flush by default");
        {
            let _guard = FlushToZero::new();
            assert_eq!(tiny(), 0.0);
            // a guard inside another leaves the mode as the outer one had it
            drop(FlushToZero::new());
            assert_eq!(tiny(), 0.0);
        }
        assert!(tiny().is_subnormal(), "the mode was put back");
    }

    #[test]
    fn flushing_state_only_removes_what_is_far_below_hearing() {
        assert_eq!(flush(1e-16), 0.0);
        assert_eq!(flush(-1e-300), 0.0);
        assert_eq!(flush(1e-10), 1e-10);
        assert_eq!(flush32(f32::MIN_POSITIVE / 2.0), 0.0);
        assert_eq!(flush32(-1e-20), 0.0);
        assert_eq!(flush32(1e-6), 1e-6);
    }
}
