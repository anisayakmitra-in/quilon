// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0
//
// Quilon runtime library (`quilon-rt`). Copyright (C) 2026 Assaf Sapir.
//
// This crate is free software licensed under version 2 of the GNU General
// Public License (see LICENSE.md), WITH the Quilon runtime-library exception —
// a Classpath-style linking exception (see LICENSE-EXCEPTION.md). The exception
// means that programs you compile with Quilon, into which this runtime is
// linked or embedded, are NOT placed under the GPL by that linking and may be
// licensed under any terms. It frees only the compiled output: this crate's own
// source remains GPLv2, so a fork of `quilon-rt` stays GPLv2.

//! Quilon runtime intrinsics — linked into every compiled Quilon program.
//!
//! These are `#[unsafe(no_mangle)] extern "C"` symbols so they resolve identically
//! from the in-process LLVM JIT (`quilon run`, via `add_global_mapping`) and from
//! ahead-of-time-linked native executables (`quilon compile` -> `llc` -> `gcc`,
//! linking `libquilon_rt.a`). The code generator declares matching external
//! prototypes and emits calls to these names; see `CodeGenerator::get_intrinsic`.
//!
//! This crate is built as both a `staticlib` (`libquilon_rt.a`, for AOT linking)
//! and an `rlib` (so the `quilon` binary embeds the same symbols for the JIT).
//!
//! The intrinsics are grouped by the surface they back: [`io`] (core.io — the one
//! genuinely lib-aligned module), [`text`] (the built-in `Text` type), [`process`]
//! (general process/runtime-lifecycle primitives: `__exit` and the entry-point
//! `argv`/`envp` conversions), and [`mem`] (general memory primitives: allocation,
//! GC, the shared `QlSlice` ABI type, bounds-check failure). Each `#[no_mangle]`
//! intrinsic is re-exported at the crate root so callers reach it as
//! `quilon_rt::__name` regardless of which module defines it.
//!
//! Memory is managed by the Boehm conservative GC (libgc); the `#[link(name = "gc")]`
//! binding lives in [`mem`]. libgc must be installed (`libgc-dev` / `gc`). When
//! linking an AOT binary with gcc, pass `-lgc` explicitly (the `#[link]` directive
//! only drives rustc's own links, not a downstream gcc invocation).

pub mod io;
pub mod mem;
pub mod process;
pub mod text;

pub use io::{__print_text_fd, __write_bytes};
pub use mem::{__alloc, __gc_init, __index_fail};
pub use process::{__argv_to_text_array, __envp_to_pairs, __exit};
pub use text::{
    __bool_to_text, __num_to_text, __text_cmp, __text_contains, __text_index_of, __text_length,
    __text_replace_all, __text_replace_n, __text_slice, __text_split, __text_to_lower,
    __text_to_upper, __text_trim_end, __text_trim_start,
};

use mem::QlSlice;
use std::os::raw::{c_char, c_int, c_void};

/// Force every runtime intrinsic to be RETAINED in the `staticlib` archive, even
/// though nothing in this crate calls them (they are only ever called from the
/// LLVM IR the code generator emits, which rustc never sees). Without an in-crate
/// reference, the staticlib's link step can dead-strip an intrinsic — observed in
/// CI as `undefined reference to __text_cmp` during AOT linking while the JIT (which
/// maps symbols by address) was unaffected. The `#[used]` table is a reachability
/// root that pins all of them. `#[used]` only guarantees retention when its
/// references stay within the intrinsic's own codegen unit, so the crate is compiled
/// as a single codegen unit (`codegen-units = 1` for `quilon-rt` in the workspace
/// `Cargo.toml`) — do not remove that override, or a multi-CGU split scatters the
/// intrinsics away from this table and some are nondeterministically dropped again.
/// (The AOT link also wraps the archive in `--whole-archive`, which pulls every
/// object already in the archive; keeping the intrinsics IN the archive is this
/// table + one codegen unit.)
// Function pointers transmuted to a common fn-pointer type — `Sync`,
// const-constructible, and each entry pins its intrinsic. Kept as a `#[used]`
// reachability root so the staticlib link never dead-strips an intrinsic that is
// only ever called from generated LLVM IR (never from Rust). All entries are plain
// `extern "C"` fn items; the transmute only erases their (ABI-compatible) parameter
// lists for storage — the pointers are never called through this array.
type RtFn = unsafe extern "C" fn();
// Each `transmute` only erases an (ABI-irrelevant) parameter list to a common
// fn-pointer type for storage; the entries are never called through this array.
#[allow(clippy::missing_transmute_annotations)]
#[used]
static QUILON_RT_INTRINSICS: [RtFn; 22] = unsafe {
    [
        core::mem::transmute(__gc_init as extern "C" fn()),
        core::mem::transmute(__num_to_text as extern "C" fn(f64) -> QlSlice),
        core::mem::transmute(__bool_to_text as extern "C" fn(i64) -> QlSlice),
        core::mem::transmute(__exit as extern "C" fn(c_int) -> !),
        core::mem::transmute(__index_fail as extern "C" fn(f64, i64) -> !),
        core::mem::transmute(__alloc as extern "C" fn(i64) -> *mut c_void),
        core::mem::transmute(__text_length as extern "C" fn(*const u8, i64) -> i64),
        core::mem::transmute(__text_cmp as extern "C" fn(*const u8, i64, *const u8, i64) -> i32),
        core::mem::transmute(__write_bytes as extern "C" fn(i64, *const u8, i64) -> i64),
        core::mem::transmute(__print_text_fd as extern "C" fn(i64, *const c_char)),
        core::mem::transmute(
            __argv_to_text_array as extern "C" fn(i64, *const *const c_char) -> QlSlice,
        ),
        core::mem::transmute(__envp_to_pairs as extern "C" fn(*const *const c_char) -> QlSlice),
        core::mem::transmute(__text_trim_start as extern "C" fn(*const u8, i64) -> QlSlice),
        core::mem::transmute(__text_trim_end as extern "C" fn(*const u8, i64) -> QlSlice),
        core::mem::transmute(__text_to_upper as extern "C" fn(*const u8, i64) -> QlSlice),
        core::mem::transmute(__text_to_lower as extern "C" fn(*const u8, i64) -> QlSlice),
        core::mem::transmute(
            __text_contains as extern "C" fn(*const u8, i64, *const u8, i64) -> i64,
        ),
        core::mem::transmute(
            __text_index_of as extern "C" fn(*const u8, i64, *const u8, i64) -> i64,
        ),
        core::mem::transmute(
            __text_replace_all
                as extern "C" fn(*const u8, i64, *const u8, i64, *const u8, i64) -> QlSlice,
        ),
        core::mem::transmute(
            __text_replace_n
                as extern "C" fn(*const u8, i64, *const u8, i64, *const u8, i64, i64) -> QlSlice,
        ),
        core::mem::transmute(__text_slice as extern "C" fn(*const u8, i64, i64, i64) -> QlSlice),
        core::mem::transmute(
            __text_split as extern "C" fn(*const u8, i64, *const u8, i64) -> QlSlice,
        ),
    ]
};

// Shared unit-test support. `GC_LOCK` is taken by GC-touching tests in more than one
// module; the `QlSlice` inspection helpers back the `text` tests. Both live here at the
// crate root so a single owner serves every module's test block.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::mem::QlSlice;
    use std::sync::Mutex;

    // libgc's `GC_init`/`GC_malloc` are not safe to invoke from several threads at
    // once; cargo runs tests in parallel, so every test that initializes/allocates
    // through the GC takes this lock first (mirrors `jit`'s JIT_LOCK).
    pub(crate) static GC_LOCK: Mutex<()> = Mutex::new(());

    /// View a `QlSlice` `Text` result as a `&str` (its GC-owned bytes). Takes the
    /// `QlSlice` by value (it is `Copy`) so the returned `&str` borrows the underlying
    /// GC buffer, not the (temporary) struct.
    pub(crate) unsafe fn slice_str<'a>(s: QlSlice) -> &'a str {
        let bytes = unsafe { std::slice::from_raw_parts(s.data as *const u8, s.len as usize) };
        std::str::from_utf8(bytes).unwrap()
    }

    pub(crate) fn text_of(s: &str) -> (*const u8, i64) {
        (s.as_ptr(), s.len() as i64)
    }

    /// Collect a `[]Text` `QlSlice` result into owned `String`s. Shared by the split tests.
    pub(crate) fn split_parts(s: &QlSlice) -> Vec<String> {
        let parts = unsafe { std::slice::from_raw_parts(s.data as *const QlSlice, s.len as usize) };
        parts
            .iter()
            .map(|p| unsafe { slice_str(*p) }.to_string())
            .collect()
    }
}
