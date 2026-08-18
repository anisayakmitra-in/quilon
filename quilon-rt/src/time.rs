// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Runtime intrinsics backing the `core.time` corelib module.
//!
//! `@sleep(seconds) -> $` is an effect-only *pause*: used as a statement it waits right
//! there on the current fiber and then execution continues, so effects sequence in program
//! order. It carries no value — the deferred/overlap story arrives with a value-returning
//! primitive (`@read`) later. `now()` is a plain (non-`@`, never-parking) monotonic clock
//! read that lets a program measure how long a pause actually took.
//!
//! `__sleep` is the FFI wrapper over [`crate::scheduler::sleep`] (the fiber-park impl lives
//! there); it must run on a scheduler fiber, which the generated entry does (see
//! `crate::scheduler::__run_fiber_main`).

use crate::scheduler;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// `@sleep(seconds)`: pause the current fiber for `seconds` seconds (a fractional `Num`,
/// like Python's `time.sleep`), then continue. Yields `$` (Unit).
#[unsafe(no_mangle)]
pub extern "C" fn __sleep(seconds: f64) {
    scheduler::sleep(Duration::from_secs_f64(seconds.max(0.0)));
}

/// `now()`: seconds on a MONOTONIC clock, as a fractional `Num`. The reference point is
/// arbitrary (first call), so only *differences* between two `now()` readings are
/// meaningful — which is exactly what measures an elapsed duration, immune to wall-clock
/// jumps (NTP steps, DST). A plain intrinsic, not an `@` primitive: reading the clock is
/// instant and never parks.
#[unsafe(no_mangle)]
pub extern "C" fn __now() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}
