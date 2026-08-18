// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Deferred values — the runtime half of Quilon's colorless implicit-futures model.
//!
//! An `@` primitive (the first is `@sleep`) does not block: it **launches** its IO on
//! a fresh task fiber and returns a *deferred* handle immediately, so two independent
//! launches overlap with nothing written. The handle is [`Deferred`], a small
//! GC-allocated cell the task fills in when it finishes. A deferred value is **forced**
//! — the forcing fiber parks until the task completes, then reads the memoized result —
//! only at a strict operation (arithmetic, comparison, output, a native call, the `^`
//! exit). Forcing is idempotent: once `ready`, later forces just read `value`.
//!
//! Structured scope: a `< >` block joins every task it launched before it returns, so a
//! launched effect never silently vanishes. [`__scope_enter`]/[`__scope_join`] bracket a
//! block; [`__sleep_launch`] registers each new task in the innermost open scope, and the
//! join forces any that were never forced.
//!
//! The whole tier is single-threaded and cooperative (one scheduler thread), so the
//! `Deferred` cell needs no synchronization: a task's write to `ready`/`value` and a
//! forcing fiber's read never run at the same instant.

use crate::mem::__alloc;
use crate::scheduler;
use std::cell::RefCell;
use std::os::raw::{c_char, c_int};
use std::time::Duration;

/// A deferred scalar result. `@`-primitives at 0.9 return `Num`, so the payload is one
/// `f64`; deferred composites (pointer-tagged `Text`/records/arrays) are a follow-up.
/// GC-allocated so the conservative stack scan keeps it alive while a fiber holds the
/// handle, and referenced by raw pointer from both the launching Quilon code and the
/// task fiber's closure.
#[repr(C)]
pub struct Deferred {
    /// Whether `task` has finished and `value` is the real result.
    ready: bool,
    /// The task fiber computing this value; valid to [`scheduler::join`] on while
    /// `!ready`. Meaningless once `ready`.
    task: usize,
    /// The computed result. Undefined until `ready`.
    value: f64,
}

thread_local! {
    /// Stack of open `< >` scopes, innermost last. Each holds the tasks launched
    /// directly inside it, to force-join at its close. A launch with no open scope
    /// (an `@` call outside any block) is joined only if it is forced.
    static SCOPES: RefCell<Vec<Vec<*mut Deferred>>> = const { RefCell::new(Vec::new()) };
}

/// Launch `@sleep(ms)`: spawn a task fiber that sleeps `ms` milliseconds and then yields
/// `ms` as the value, and return the deferred handle immediately (the caller does NOT
/// block). Two `@sleep` launches therefore overlap. The handle is registered in the
/// innermost open scope so the enclosing block joins it even if it is never forced.
#[unsafe(no_mangle)]
pub extern "C" fn __sleep_launch(ms: f64) -> *mut Deferred {
    let d = __alloc(std::mem::size_of::<Deferred>() as i64) as *mut Deferred;
    // SAFETY: `d` is a fresh, correctly-sized, 8-aligned GC allocation for `Deferred`.
    unsafe {
        (*d).ready = false;
        (*d).value = 0.0;
    }
    let task = scheduler::spawn_returning(move || {
        scheduler::sleep(Duration::from_millis(ms.max(0.0) as u64));
        // SAFETY: `d` is live — the launching fiber holds the handle and any parked
        // forcer holds it too, so the GC has not freed it; single-threaded, so this
        // write cannot race the forcer's read.
        unsafe {
            (*d).value = ms;
            (*d).ready = true;
        }
    });
    // SAFETY: as above; set before any fiber can force `d` (no fiber runs until this
    // one yields, which happens no earlier than the first force).
    unsafe { (*d).task = task };
    register_in_scope(d);
    d
}

/// Force a deferred `Num`: if the task has not finished, park until it does (join), then
/// read the memoized value. Idempotent — a ready handle reads straight through. Emitted
/// by codegen at every force-set site that must read a deferred `Num`'s concrete bits.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __force_num(d: *mut Deferred) -> f64 {
    // SAFETY: `d` is a live `Deferred` handle produced by an `@` launch.
    unsafe {
        if !(*d).ready {
            scheduler::join((*d).task);
        }
        (*d).value
    }
}

/// Open a `< >` scope: begin collecting the tasks launched inside it.
#[unsafe(no_mangle)]
pub extern "C" fn __scope_enter() {
    SCOPES.with(|s| s.borrow_mut().push(Vec::new()));
}

/// Close the innermost `< >` scope, forcing every task it launched that was not already
/// forced. This is the structured-concurrency guarantee: a launched effect is always
/// joined before its block returns.
#[unsafe(no_mangle)]
pub extern "C" fn __scope_join() {
    let tasks = SCOPES.with(|s| s.borrow_mut().pop()).unwrap_or_default();
    for d in tasks {
        // SAFETY: every registered pointer is a live `Deferred` from a launch in this
        // scope; forcing it joins its task if still pending.
        let _ = __force_num(d);
    }
}

/// Record a freshly-launched task in the innermost open scope (if any).
fn register_in_scope(d: *mut Deferred) {
    SCOPES.with(|s| {
        if let Some(scope) = s.borrow_mut().last_mut() {
            scope.push(d);
        }
    });
}

/// The C `main` wrapper the code generator emits calls this to run the program's entry
/// on a scheduler fiber (only when the program uses deferral — pure programs call the
/// entry directly, unchanged). `entry` is the generated `__ql_entry` thunk with the C
/// `main` signature; its `i32` result is the program's exit code. Runs `entry` as the
/// seed fiber so any `@` primitive it reaches has a fiber to park on.
#[unsafe(no_mangle)]
pub extern "C" fn __run_fiber_main(
    entry: extern "C" fn(c_int, *const *const c_char, *const *const c_char) -> c_int,
    argc: c_int,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    // The seed fiber's exit code, moved out through a heap cell (the closure must be
    // `'static`; a `Box` leaked and reclaimed avoids borrowing a stack local).
    let code: *mut c_int = Box::into_raw(Box::new(0));
    // Raw pointers (`argv`/`envp`/`code`) are `Copy` and carry no lifetime, so the
    // closure is `'static`; single-threaded, so no `Send` is needed.
    scheduler::run(move || {
        let result = entry(argc, argv, envp);
        // SAFETY: `code` is the live box allocated just above; only this fiber writes it.
        unsafe { *code = result };
    });
    // SAFETY: reclaim the box the seed fiber wrote; the run has finished, so no fiber
    // still references it.
    let boxed = unsafe { Box::from_raw(code) };
    *boxed
}
