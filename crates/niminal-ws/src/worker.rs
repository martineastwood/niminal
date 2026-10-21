//! A background thread that runs until dropped.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

/// Runs `body` on its own thread; dropping the worker tells it to stop and waits for it.
/// `body` should return once the flag it is given turns true.
pub struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn wake_handle(&self) -> thread::Thread {
        self.thread
            .as_ref()
            .expect("a running worker")
            .thread()
            .clone()
    }

    pub fn spawn(body: impl FnOnce(&AtomicBool) + Send + 'static) -> Worker {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        Worker {
            stop,
            thread: Some(thread::spawn(move || body(&flag))),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}
