//! The wall-clock watchdog's clock (`runtime/30` §7 `max_wall_clock`, R-RUN-24, D-10, D-51): injected, so tests drive it
//! with a fake one instead of real time (R-QA-02). It lives here and not in the interpreter, which reaches no clock
//! (R-RUN-05); the interpreter only asks whether the run has been stopped.

use std::fmt::Debug;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use velme_interp::Interrupt;

use crate::sched::Options;

/// A source of time. Only differences between two readings mean anything.
pub trait Clock: Debug + Send + Sync {
    /// The time since some fixed start.
    fn now(&self) -> Duration;
}

/// The host's monotonic clock.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    /// A clock that starts now.
    pub fn new() -> SystemClock {
        SystemClock { origin: Instant::now() }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// The wall-clock watchdog of one run (`runtime/30` §7, D-10): the clock, when the run started by it, how long it may
/// take, and whether it has run out. Once out of time it stays out, so everything that shares it stops.
#[derive(Debug)]
pub(crate) struct Watchdog {
    clock: Arc<dyn Clock>,
    started: Duration,
    max_wall_clock: Duration,
    expired: AtomicBool,
}

impl Watchdog {
    /// A watchdog for a run that starts now by the clock of `options`.
    pub(crate) fn start(options: &Options) -> Watchdog {
        Watchdog {
            started: options.clock.now(),
            clock: Arc::clone(&options.clock),
            max_wall_clock: options.max_wall_clock,
            expired: AtomicBool::new(false),
        }
    }

    /// Whether the run has been going longer than `max_wall_clock`.
    pub(crate) fn timed_out(&self) -> bool {
        if self.expired.load(Ordering::SeqCst) {
            return true;
        }
        if self.clock.now().saturating_sub(self.started) > self.max_wall_clock {
            self.expired.store(true, Ordering::SeqCst);
            return true;
        }
        false
    }

    /// The watchdog as the interpreter asks it, every so often, whether to stop.
    pub(crate) fn interrupt(self: &Arc<Self>) -> Interrupt {
        let watchdog = Arc::clone(self);
        Interrupt::new(move || watchdog.timed_out())
    }
}
