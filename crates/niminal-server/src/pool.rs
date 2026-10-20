//! A few threads that share out the jobs of a block of audio. They never allocate and never wait on a
//! lock while a block is rendering: they spin briefly for the next batch of jobs, then park.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use niminal_engine::Fork;

type Job<'a> = &'a (dyn Fn(usize) + Sync);

/// How long an idle worker spins, looking for a new batch, before it parks. Blocks come one after
/// another while there is audio to make, so waiting a little saves the cost of waking a thread.
const SPIN: Duration = Duration::from_micros(200);

/// What the measured cost of sharing out a wave is multiplied by, to allow for threads that were
/// asleep, which the measurement (made with warm threads) does not see.
const OVERHEAD_MARGIN: u64 = 3;

/// When to share a wave of voices among threads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Policy {
    /// When the time it saves is more than the measured cost of doing it.
    #[default]
    Auto,
    /// Whenever there are two or more chunks.
    Always,
    Never,
    /// When the wave would take at least this many microseconds to make one chunk after another.
    AtLeastMicros(u64),
}

impl std::str::FromStr for Policy {
    type Err = String;
    fn from_str(s: &str) -> Result<Policy, String> {
        match s {
            "auto" => Ok(Policy::Auto),
            "always" => Ok(Policy::Always),
            "never" => Ok(Policy::Never),
            n => n
                .parse()
                .map(Policy::AtLeastMicros)
                .map_err(|_| format!("`{s}`: use auto, always, never or a number of microseconds")),
        }
    }
}

struct Shared {
    /// The batch number (16 bits), the number of jobs in it (16 bits) and the next job to take (32
    /// bits), changed together so that a worker can only take a job that really belongs to the batch
    /// it means, whatever else it has read about it since.
    ticket: AtomicU64,
    done: AtomicUsize,
    /// Points at the batch's `Job`, which `run` keeps alive until every job is done.
    job: AtomicPtr<()>,
    sleepers: AtomicUsize,
    quit: AtomicBool,
    /// A job panicked, and its panic is in `panic`. Checked with an atomic so that a run that goes
    /// well never takes a lock, which may allocate the first time.
    panicked: AtomicBool,
    panic: Mutex<Option<Box<dyn Any + Send>>>,
}

fn batch_of(ticket: u64) -> u16 {
    (ticket >> 48) as u16
}

impl Shared {
    /// The next job of `batch`, if it has one left.
    fn claim(&self, batch: u16) -> Option<usize> {
        let mut t = self.ticket.load(Acquire);
        loop {
            let (total, next) = ((t >> 32) as u16 as usize, (t & 0xffff_ffff) as usize);
            if batch_of(t) != batch || next >= total {
                return None;
            }
            match self.ticket.compare_exchange_weak(t, t + 1, AcqRel, Acquire) {
                Ok(_) => return Some(next),
                Err(now) => t = now,
            }
        }
    }

    /// Run jobs of `batch` until there are none left to take. With no `job`, the batch's own is used.
    fn work(&self, batch: u16, job: Option<Job>) {
        while let Some(i) = self.claim(batch) {
            // SAFETY: a job of this batch was just taken and is not finished, so `run` has not
            // returned, and the pointer it stored before publishing the batch points at its `Job`.
            let job = job.unwrap_or_else(|| unsafe { *(self.job.load(Acquire) as *const Job<'static>) });
            if let Err(e) = catch_unwind(AssertUnwindSafe(|| job(i))) {
                self.panic
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get_or_insert(e);
                self.panicked.store(true, Release);
            }
            self.done.fetch_add(1, AcqRel);
        }
    }
}

pub struct Pool {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    /// Whether a batch is running. Batches are one at a time; a second caller runs its jobs itself.
    busy: AtomicBool,
    batches: AtomicU32,
    policy: Policy,
    start: Instant,
    /// What sharing out a wave costs, in nanoseconds, as measured when the pool started.
    overhead_ns: u64,
}

impl Pool {
    /// `workers` threads besides the caller's. `init` runs first on each, for scheduling hints.
    pub fn new(workers: usize, policy: Policy, init: impl Fn() + Send + Sync + 'static) -> Pool {
        let shared = Arc::new(Shared {
            ticket: AtomicU64::new(0),
            done: AtomicUsize::new(0),
            job: AtomicPtr::new(std::ptr::null_mut()),
            sleepers: AtomicUsize::new(0),
            quit: AtomicBool::new(false),
            panicked: AtomicBool::new(false),
            panic: Mutex::new(None),
        });
        let init = Arc::new(init);
        let threads = (0..workers)
            .map(|n| {
                let (shared, init) = (shared.clone(), init.clone());
                thread::Builder::new()
                    .name(format!("niminal-dsp-{n}"))
                    .spawn(move || {
                        init();
                        worker(&shared);
                    })
                    .expect("spawn a DSP worker")
            })
            .collect();
        let mut pool = Pool {
            shared,
            threads,
            busy: AtomicBool::new(false),
            batches: AtomicU32::new(0),
            policy,
            start: Instant::now(),
            overhead_ns: 0,
        };
        if policy == Policy::Auto && workers > 0 {
            pool.overhead_ns = pool.measure_overhead() * OVERHEAD_MARGIN;
        }
        pool
    }

    /// How long, on average, a wave of one empty job for each thread takes to share out and finish.
    fn measure_overhead(&self) -> u64 {
        let wave = |times: u64| {
            for _ in 0..times {
                self.run(self.threads.len() + 1, &|_| {});
            }
        };
        wave(200);
        let (started, rounds) = (self.now(), 2000);
        wave(rounds);
        (self.now() - started) / rounds
    }

    /// What sharing out a wave costs, in nanoseconds, as measured when the pool started (and with the
    /// safety margin on it).
    pub fn overhead_ns(&self) -> u64 {
        self.overhead_ns
    }

    pub fn workers(&self) -> usize {
        self.threads.len()
    }
}

fn worker(s: &Shared) {
    let mut last = 0u16;
    loop {
        let (mut spins, mut idle_since) = (0u32, Instant::now());
        let batch = loop {
            if s.quit.load(Acquire) {
                return;
            }
            let batch = batch_of(s.ticket.load(Acquire));
            if batch != last {
                break batch;
            }
            spins += 1;
            if spins % 64 != 0 || idle_since.elapsed() < SPIN {
                std::hint::spin_loop();
                continue;
            }
            s.sleepers.fetch_add(1, SeqCst);
            // run's store and this load are both SeqCst, so one of the two sees the other
            if batch_of(s.ticket.load(SeqCst)) == last && !s.quit.load(SeqCst) {
                thread::park();
            }
            s.sleepers.fetch_sub(1, SeqCst);
            idle_since = Instant::now();
        };
        last = batch;
        s.work(batch, None);
    }
}

impl Fork for Pool {
    fn now(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64
    }

    fn worth_it(&self, work_ns: u64, jobs: usize) -> bool {
        if jobs < 2 {
            return false;
        }
        match self.policy {
            Policy::Always => true,
            Policy::Never => false,
            Policy::AtLeastMicros(us) => work_ns >= us * 1000,
            Policy::Auto => {
                let threads = jobs.min(self.threads.len() + 1) as u64;
                work_ns - work_ns / threads > self.overhead_ns
            }
        }
    }

    fn run(&self, jobs: usize, job: &(dyn Fn(usize) + Sync)) {
        if self.busy.swap(true, Acquire) {
            return (0..jobs).for_each(job);
        }
        struct Idle<'a>(&'a AtomicBool);
        impl Drop for Idle<'_> {
            fn drop(&mut self) {
                self.0.store(false, Release);
            }
        }
        let _busy = Idle(&self.busy);
        assert!(jobs <= usize::from(u16::MAX), "too many jobs for one batch");
        let s = &*self.shared;
        let batch = self.batches.load(Relaxed).wrapping_add(1) as u16;
        self.batches.store(u32::from(batch), Relaxed);
        let slot: Job = job;
        s.job.store(&slot as *const Job as *mut (), Release);
        s.done.store(0, Release);
        s.ticket
            .store(u64::from(batch) << 48 | (jobs as u64) << 32, SeqCst);
        if s.sleepers.load(SeqCst) > 0 {
            self.threads.iter().for_each(|t| t.thread().unpark());
        }
        s.work(batch, Some(job));
        let mut waited = 0u32;
        while s.done.load(Acquire) < jobs {
            waited += 1;
            if waited < 20_000 {
                std::hint::spin_loop();
            } else {
                thread::yield_now();
            }
        }
        if s.panicked.swap(false, AcqRel) {
            let panic = s.panic.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(panic) = panic {
                resume_unwind(panic);
            }
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.quit.store(true, SeqCst);
        self.threads.iter().for_each(|t| t.thread().unpark());
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_job_runs_once_over_many_batches() {
        let pool = Pool::new(3, Policy::Always, || {});
        let counts: Vec<AtomicU32> = (0..16).map(|_| AtomicU32::new(0)).collect();
        let mut expected = [0u32; 16];
        for round in 0..5000usize {
            let jobs = 1 + round % 16;
            pool.run(jobs, &|i| {
                counts[i].fetch_add(1, Relaxed);
            });
            expected[..jobs].iter_mut().for_each(|e| *e += 1);
            let seen: Vec<u32> = counts.iter().map(|c| c.load(Relaxed)).collect();
            assert_eq!(seen, expected);
        }
    }

    #[test]
    fn workers_wake_after_parking() {
        let pool = Pool::new(2, Policy::Always, || {});
        thread::sleep(std::time::Duration::from_millis(50));
        let sum = AtomicU32::new(0);
        pool.run(8, &|i| {
            sum.fetch_add(i as u32, Relaxed);
        });
        assert_eq!(sum.load(Relaxed), 28);
    }

    #[test]
    fn a_panicking_job_surfaces_on_the_caller_and_the_pool_carries_on() {
        let pool = Pool::new(2, Policy::Always, || {});
        let caught = catch_unwind(AssertUnwindSafe(|| {
            pool.run(8, &|i| assert_ne!(i, 5, "boom"));
        }));
        assert!(caught.is_err());
        let ran = AtomicU32::new(0);
        pool.run(8, &|_| {
            ran.fetch_add(1, Relaxed);
        });
        assert_eq!(ran.load(Relaxed), 8);
    }
}
