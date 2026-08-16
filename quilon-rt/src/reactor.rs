// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The reactor: a thin wrapper over a `mio` poll loop. It drives two kinds of
//! readiness for the scheduler:
//!
//!  * timed waits — the nearest sleep deadline becomes the poll timeout, so the
//!    scheduler wakes in time to resume due `sleep`ers; and
//!  * socket readiness — non-blocking TCP sources ([`crate::net`]) are registered
//!    with a unique [`Token`]; when `poll` reports a token ready the scheduler
//!    resumes the one fiber parked on it.
//!
//! A single `Poll::poll` services both: the timeout bounds how long it blocks, and
//! any socket that becomes ready before then returns it early. `EINTR` and spurious
//! wakeups are harmless — the scheduler re-checks timers and only wakes fibers whose
//! token actually fired.

use mio::event::Source;
use mio::{Events, Interest, Poll, Token};
use std::io;
use std::time::Duration;

pub struct Reactor {
    poll: Poll,
    events: Events,
    /// Monotonic token allocator. Tokens are never reused; a `usize` counter is
    /// ample and keeps the token/fiber map unambiguous even after sources close.
    next_token: usize,
}

impl Reactor {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            poll: Poll::new()?,
            events: Events::with_capacity(64),
            next_token: 0,
        })
    }

    /// Hand out a fresh, never-reused token for a new source.
    pub fn alloc_token(&mut self) -> Token {
        let token = Token(self.next_token);
        self.next_token += 1;
        token
    }

    pub fn register(
        &self,
        source: &mut impl Source,
        token: Token,
        interest: Interest,
    ) -> io::Result<()> {
        self.poll.registry().register(source, token, interest)
    }

    pub fn reregister(
        &self,
        source: &mut impl Source,
        token: Token,
        interest: Interest,
    ) -> io::Result<()> {
        self.poll.registry().reregister(source, token, interest)
    }

    pub fn deregister(&self, source: &mut impl Source) -> io::Result<()> {
        self.poll.registry().deregister(source)
    }

    /// Block until `timeout` elapses or a registered source is ready. `None` blocks
    /// indefinitely; the scheduler passes `None` only when fibers are parked on
    /// sockets with no pending timer.
    pub fn wait(&mut self, timeout: Option<Duration>) {
        self.events.clear();
        let _ = self.poll.poll(&mut self.events, timeout);
    }

    /// Tokens whose sources became ready in the last [`wait`](Self::wait). The
    /// scheduler maps each back to the fiber parked on it.
    pub fn ready_tokens(&self) -> impl Iterator<Item = Token> + '_ {
        self.events.iter().map(|event| event.token())
    }
}
