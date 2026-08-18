// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The `@` leaf-IO-primitive tier — the first Quilon-visible surface of the concurrency
//! runtime. The first primitive is `@sleep(secs) -> $`, an effect-only *pause*: used as a
//! statement it waits right there on the current fiber and then execution continues, so
//! effects sequence in program order. It carries no value, so there is nothing to defer or
//! force yet; a value-returning primitive (`@read`) and the deferred/overlap machinery land
//! on top of this later.
//!
//! Running the program's entry on a scheduler fiber ([`__run_fiber_main`]) is what gives an
//! `@` primitive a fiber to park on. Only a program that uses an `@` primitive is wrapped
//! this way; a pure program calls its entry directly and is unchanged.

use crate::scheduler;
use std::cell::Cell;
use std::os::raw::{c_char, c_int};
use std::rc::Rc;
use std::time::Duration;

/// `@sleep(secs)`: pause the current fiber for `secs` seconds (a fractional `Num`, like
/// Python's `time.sleep`), then continue. Yields `$` (Unit). Must run on a scheduler fiber
/// (the generated entry does — see [`__run_fiber_main`]).
#[unsafe(no_mangle)]
pub extern "C" fn __sleep(secs: f64) {
    scheduler::sleep(Duration::from_secs_f64(secs.max(0.0)));
}

/// The C `main` wrapper the code generator emits calls this to run the program's entry on a
/// scheduler fiber (only when the program uses an `@` primitive — pure programs call the
/// entry directly, unchanged). `entry` is the generated `__ql_entry` thunk with the C
/// `main` signature; its `i32` result is the program's exit code. Running the entry as the
/// seed fiber gives any `@` primitive it reaches a fiber to park on.
#[unsafe(no_mangle)]
pub extern "C" fn __run_fiber_main(
    entry: extern "C" fn(c_int, *const *const c_char, *const *const c_char) -> c_int,
    argc: c_int,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    // The seed fiber writes its exit code out through a shared cell. `Rc<Cell<_>>` is
    // `'static` (no borrow of a stack local) and `Clone`, which is all the closure needs:
    // the tier is single-threaded, so no `Send`/synchronization is involved.
    let code = Rc::new(Cell::new(0));
    let code_writer = code.clone();
    scheduler::run(move || {
        code_writer.set(entry(argc, argv, envp));
    });
    code.get()
}
