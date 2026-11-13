//! Keep a worker alive: if it panics, start it again instead of leaving the app running with
//! nothing behind it (a dead clock link used to mean "not connected" until the app was
//! restarted by hand, with the serial port still held).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

/// A worker that ran this long before failing wasn't in a crash loop: the wait starts over.
const SETTLED: Duration = Duration::from_secs(300);
const MAX_WAIT: Duration = Duration::from_secs(60);

/// Run `work` until it returns normally (the app is stopping) or `stopping()` says so. After a
/// panic (which the panic hook logs), wait `first_wait`, then twice as long each time it keeps
/// failing, up to a minute; `sleep` does the waiting and `on_restart(name, wait)` is told.
pub fn run(
    name: &str,
    first_wait: Duration,
    stopping: impl Fn() -> bool,
    sleep: impl Fn(Duration),
    on_restart: impl Fn(&str, Duration),
    work: impl Fn(),
) {
    let mut wait = first_wait;
    while !stopping() {
        let started = Instant::now();
        if catch_unwind(AssertUnwindSafe(&work)).is_ok() {
            return;
        }
        if started.elapsed() > SETTLED {
            wait = first_wait;
        }
        on_restart(name, wait);
        sleep(wait);
        wait = (wait * 2).min(MAX_WAIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn a_panicking_worker_is_started_again_with_growing_waits() {
        let runs = Cell::new(0);
        let waits = RefCell::new(Vec::new());
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // keep the test output quiet
        run(
            "worker",
            Duration::from_secs(2),
            || false,
            |d| waits.borrow_mut().push(d.as_secs()),
            |_, _| {},
            || {
                runs.set(runs.get() + 1);
                if runs.get() <= 3 {
                    panic!("boom");
                }
            },
        );
        std::panic::set_hook(hook);
        assert_eq!(runs.get(), 4, "three failures, then it ran to the end");
        assert_eq!(*waits.borrow(), vec![2, 4, 8]);
    }

    #[test]
    fn nothing_is_restarted_once_the_app_is_stopping() {
        let runs = Cell::new(0);
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        run(
            "worker",
            Duration::from_secs(2),
            || runs.get() >= 1,
            |_| {},
            |_, _| {},
            || {
                runs.set(runs.get() + 1);
                panic!("boom");
            },
        );
        std::panic::set_hook(hook);
        assert_eq!(runs.get(), 1);
    }
}
