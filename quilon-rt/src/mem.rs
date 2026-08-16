// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Internal runtime primitives with no `core.*` language home: allocation and the
//! Boehm-GC binding (`__alloc`, `__gc_init`), the shared `QlSlice` `{ ptr, len }`
//! ABI type and its `alloc_text` helper, the `format_num` render helper, and the
//! fail-loud `__index_fail` bounds-check primitive (checked `arr[i]` has no
//! `core.*` module, so it lives in this internal tier). This tier is where the
//! future fiber scheduler and reactor will also live.

use crate::io::write_to_fd;
use crate::process::__exit;
use std::os::raw::c_void;

// Link the Boehm GC and tie it to these symbol references so the linker keeps
// libgc for every target (binary, tests, JIT harness) regardless of `--as-needed`
// ordering. libgc must be installed (`libgc-dev` / `gc`); CI installs it.
#[link(name = "gc")]
unsafe extern "C" {
    fn GC_malloc(size: usize) -> *mut c_void;
    fn GC_init();
}

/// Initialize the garbage collector. Emitted as the first call in `main`.
#[unsafe(no_mangle)]
pub extern "C" fn __gc_init() {
    // Safe to call more than once; GC_init is idempotent.
    unsafe { GC_init() }
}

/// Allocate `size` bytes of GC-managed, zeroed-on-demand memory.
///
/// Returns a pointer the collector tracks; callers never free it. A non-positive
/// size yields a 1-byte allocation so the result is always a valid, unique-ish
/// pointer.
#[unsafe(no_mangle)]
pub extern "C" fn __alloc(size: i64) -> *mut c_void {
    let n = if size <= 0 { 1 } else { size as usize };
    unsafe { GC_malloc(n) }
}

/// Report an invalid array index — out of bounds, negative, or NaN — to stderr and
/// terminate with exit status 1: the fail-loud contract of checked `arr[i]` indexing.
/// `index` is the ORIGINAL f64 the program computed (pre-truncation), so the message
/// shows what the user actually asked for; `size` is the array's element count.
/// Codegen calls this from the invalid branch of every `arr[i]` bounds check.
#[unsafe(no_mangle)]
pub extern "C" fn __index_fail(index: f64, size: i64) -> ! {
    let msg = format!(
        "runtime error: array index {} out of bounds (size {})\n",
        format_num(index),
        size
    );
    write_to_fd(2, msg.as_bytes());
    __exit(1)
}

/// A Quilon `Text` value (also the representation of an array): `{ ptr data, i64 len }`,
/// matching the code generator's `ptr_len_struct_type` (`{ i8*, i64 }`). For a `Text`,
/// `data` points to `len` UTF-8 bytes; for an array, `data` points to `len` contiguous
/// element-representation values and `len` is the element count. `#[repr(C)]` so the
/// field offsets (ptr at 0, i64 at 8) match what LLVM emits.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct QlSlice {
    pub(crate) data: *const c_void,
    pub(crate) len: i64,
}

impl QlSlice {
    /// The empty slice (`{ null, 0 }`) — a zero-length `Text`/array. Returned when there
    /// is nothing to build (null/empty `argv`/`envp`).
    pub(crate) fn empty() -> QlSlice {
        QlSlice {
            data: std::ptr::null(),
            len: 0,
        }
    }
}

/// GC-allocate a `Text` whose bytes are a copy of `bytes`. The copy is owned by the GC
/// (so it outlives the C `argv`/`envp` buffers, which the program may not keep), and is
/// NUL-terminated past `len` so `print`/`eprint` (which expect a C string) work too.
pub(crate) fn alloc_text(bytes: &[u8]) -> QlSlice {
    let len = bytes.len();
    // +1 for a trailing NUL so the buffer doubles as a C string for `print`.
    let buf = __alloc(len as i64 + 1) as *mut u8;
    if !buf.is_null() && len > 0 {
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len) };
    }
    QlSlice {
        data: buf as *const c_void,
        len: len as i64,
    }
}

/// Render an `f64` the way Quilon shows a `Num`: whole values without a fractional part
/// (`5`, not `5.0`), everything else in shortest round-trip form. Shared by `__num_to_text`
/// and the `__index_fail` diagnostic.
pub(crate) fn format_num(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{}", x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::GC_LOCK;

    #[test]
    fn format_num_drops_trailing_zeros_for_whole_values() {
        assert_eq!(format_num(3.0), "3");
        assert_eq!(format_num(120.0), "120");
        assert_eq!(format_num(3.5), "3.5");
    }

    #[test]
    fn alloc_returns_usable_memory() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let p = __alloc(16) as *mut u8;
        assert!(!p.is_null());
        unsafe {
            std::ptr::write_bytes(p, 0xAB, 16);
            assert_eq!(*p, 0xAB);
        }
    }
}
