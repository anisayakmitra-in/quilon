// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The reactor: a thin wrapper over a `mio` poll loop. Today it only provides a
//! timed wait that drives the scheduler's wall clock (used by [`crate::sched`] to
//! sleep until the nearest wake deadline). No I/O sources are registered yet;
//! socket/file readiness driving fiber resume arrives with the `@` IO primitives.

use mio::{Events, Poll};
use std::io;
use std::time::Duration;

pub struct Reactor {
    poll: Poll,
    events: Events,
}

impl Reactor {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            poll: Poll::new()?,
            events: Events::with_capacity(64),
        })
    }

    /// Block until `timeout` elapses (or, later, until a registered source is
    /// ready). `None` blocks indefinitely; the scheduler only passes `None` when a
    /// fiber is waiting on readiness with no deadline — for sleep-only workloads it
    /// always passes a finite timeout. `EINTR` and spurious wakeups are harmless:
    /// the scheduler re-checks timers after every wait.
    pub fn wait(&mut self, timeout: Option<Duration>) {
        self.events.clear();
        let _ = self.poll.poll(&mut self.events, timeout);
    }
}
