// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Deferred values — the runtime half of Quilon's colorless implicit futures.
//!
//! A value-returning `@` primitive does not park the calling fiber and hand back bytes;
//! it *launches* the IO on a background fiber and returns a **promise** immediately. The
//! promise flows through the program as an ordinary value (a `Text`, here) and is *forced*
//! only when a strict primitive is about to read its concrete bytes. Forcing parks the
//! current fiber until the producing fiber has stored the result, then reads it — memoized,
//! so a second force is O(1).
//!
//! [`__read_launch`] backs `@read` (read one line from stdin): it allocates a [`Promise`],
//! spawns a reader fiber that parks on stdin readiness and fills the cell, and returns the
//! deferred [`QlSlice`] representation. [`__force_text`] is the force: park-until-ready,
//! then return the stored bytes.
//!
//! Representation (hybrid, per the concurrency design): a `Text` is a `{ ptr, i64 }`
//! `QlSlice`. A *ready* `Text` carries its byte length (`>= 0`) in the second field; a
//! *deferred* `Text` carries [`DEFERRED_SENTINEL`] (`-1`) there and the promise pointer in
//! the first — a real byte length is never negative, so the two are unambiguous. The code
//! generator forces exactly at the strict-use sites the deferred-taint pass marks, and only
//! for values that pass can be deferred; pure code never sees a sentinel and pays nothing.
//!
//! GC: the promise cell is GC-allocated so its stored `data` pointer keeps the result bytes
//! alive; the reader fiber holds the cell on its (GC-scanned) stack from launch until it
//! returns, and the forcing fiber holds it across the park — so a collection at any point in
//! the deferred lifetime finds it. `tests` proves a not-yet-resumed reader fiber's captured
//! cell survives collection.

use crate::io::write_to_fd;
use crate::mem::{__alloc, QlSlice, alloc_text};
use crate::process::__exit;
use crate::scheduler::{
    deregister_readiness, park_on_promise, park_on_readiness, register_readiness,
    reregister_readiness, spawn, wake_promise,
};
use mio::unix::SourceFd;
use mio::{Interest, Token};
use std::cell::{Cell, RefCell};
use std::io;
use std::os::raw::c_void;
use std::ptr;

/// The second (`i64`) field of a deferred `Text`'s `QlSlice`: a real byte length is never
/// negative, so `-1` unambiguously flags "the first field is a promise pointer, not data".
/// The code generator's force check compares against this exact value.
pub const DEFERRED_SENTINEL: i64 = -1;

const PENDING: i64 = 0;
const READY: i64 = 1;

/// A single deferred value's cell. GC-allocated; only ever accessed by the one producing
/// fiber (which writes it once) and the forcing fiber(s) (which read after a wake) — never
/// concurrently, since the tier is single-threaded and cooperative, so plain fields need no
/// synchronization.
#[repr(C)]
struct Promise {
    /// `PENDING` until the producer stores its result, then `READY`.
    state: i64,
    /// The result `Text`'s bytes (GC-owned) and byte length, valid once `state` is `READY`.
    data: *const c_void,
    len: i64,
    /// The `@`-call launch site (origin), reported if the IO faults — so a fault points at
    /// where the read was *called*, not merely where it was forced. Null when unknown.
    site_data: *const u8,
    site_len: i64,
}

/// `@read()`: launch a background read of one line from stdin and return the deferred
/// `Text` immediately (the calling fiber does not park here). `site_data`/`site_len`
/// describe the `@read` call site for fault reporting; either may be null/zero if unknown.
///
/// # Safety contract (upheld by the compiler)
/// `site_data` is null or points to `site_len` readable bytes that outlive the program
/// (a static string constant emitted by the code generator).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __read_launch(site_data: *const u8, site_len: i64) -> QlSlice {
    let promise = __alloc(std::mem::size_of::<Promise>() as i64) as *mut Promise;
    // SAFETY: `__alloc` returned a fresh, suitably-aligned cell of exactly this size.
    unsafe {
        (*promise).state = PENDING;
        (*promise).data = ptr::null();
        (*promise).len = 0;
        (*promise).site_data = site_data;
        (*promise).site_len = site_len;
    }
    let address = promise as usize;

    // Eager launch: the read runs whether or not the result is ever forced. The reader
    // holds `promise` on its own (GC-scanned) stack for its whole life, so the cell — and
    // the bytes it will store — stay reachable across any collection while pending.
    spawn(move || {
        let bytes = match read_stdin_line() {
            Ok(bytes) => bytes,
            // SAFETY: `promise` is the live cell this closure owns.
            Err(error) => unsafe { fail_read(promise, &error) },
        };
        let text = alloc_text(&bytes);
        // SAFETY: still the live cell; single-threaded, so this write cannot race a force.
        unsafe {
            (*promise).data = text.data;
            (*promise).len = text.len;
            (*promise).state = READY;
        }
        wake_promise(address);
    });

    QlSlice {
        data: promise as *const c_void,
        len: DEFERRED_SENTINEL,
    }
}

/// Force a deferred `Text`: park the current fiber until the promise at `promise_ptr` is
/// fulfilled, then return its bytes. Only the code generator calls this, and only after its
/// force check saw [`DEFERRED_SENTINEL`], so `promise_ptr` is always a live [`Promise`].
///
/// # Safety contract (upheld by the compiler)
/// `promise_ptr` is a pointer previously returned in the first field of a deferred
/// `__read_launch` result and is still reachable (the taint pass keeps it live to here).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __force_text(promise_ptr: *const c_void) -> QlSlice {
    let promise = promise_ptr as *const Promise;
    let address = promise as usize;
    loop {
        // Re-read every iteration: a wake is an invitation to look, not a guarantee, and the
        // producer's store happens-before our resume (cooperative single thread).
        // SAFETY: `promise` is a live cell for the whole force (see the contract).
        if unsafe { (*promise).state } == READY {
            // SAFETY: `READY` means `data`/`len` were stored and are final.
            return unsafe {
                QlSlice {
                    data: (*promise).data,
                    len: (*promise).len,
                }
            };
        }
        park_on_promise(address);
    }
}

/// Report a fatal stdin read error against the promise's launch site, then terminate the
/// process (fail-loud). A genuine IO error on stdin is neither EOF nor `WouldBlock`.
///
/// # Safety
/// `promise` is a live cell.
unsafe fn fail_read(promise: *const Promise, error: &io::Error) -> ! {
    let (site_data, site_len) = unsafe { ((*promise).site_data, (*promise).site_len) };
    let site = if site_data.is_null() || site_len <= 0 {
        "<unknown>".to_string()
    } else {
        // SAFETY: the compiler's contract: `site_len` readable bytes at `site_data`.
        let bytes = unsafe { std::slice::from_raw_parts(site_data, site_len as usize) };
        String::from_utf8_lossy(bytes).into_owned()
    };
    let message = format!("runtime error: @read at {site} failed: {error}\n");
    write_to_fd(2, message.as_bytes());
    __exit(1)
}

thread_local! {
    /// Bytes read past the newline of the previous `@read`, kept so the next `@read`
    /// continues the same stream line-by-line rather than dropping them.
    static STDIN_LEFTOVER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    /// Whether stdin (fd 0) has been switched to non-blocking yet (done once per thread).
    static STDIN_NONBLOCKING: Cell<bool> = const { Cell::new(false) };
}

const STDIN_FD: i32 = 0;

/// Read one line from stdin, parking the fiber on readiness until a newline arrives or the
/// stream ends. Returns the line WITHOUT its trailing newline (a trailing `\r` is dropped
/// too). At end-of-input with nothing buffered, returns an empty `Vec` — the documented
/// end-of-input value (`@read` yields an empty `Text` there). Bytes past the newline are
/// retained for the next call.
fn read_stdin_line() -> io::Result<Vec<u8>> {
    if let Some(line) = take_buffered_line() {
        return Ok(line);
    }
    set_stdin_nonblocking();
    let fd = STDIN_FD;
    let mut source = SourceFd(&fd);
    let token = register_readiness(&mut source, Interest::READABLE)?;
    let result = read_until_line(&mut source, token);
    deregister_readiness(&mut source);
    result
}

/// Loop reading from stdin into the leftover buffer, parking on `WouldBlock`, until a full
/// line is buffered or the stream ends. `source`/`token` are already registered readable.
fn read_until_line(source: &mut SourceFd, token: Token) -> io::Result<Vec<u8>> {
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(line) = take_buffered_line() {
            return Ok(line);
        }
        // SAFETY: `read(2)` into a valid, owned buffer of `chunk.len()` bytes.
        let count = unsafe { libc::read(STDIN_FD, chunk.as_mut_ptr() as *mut c_void, chunk.len()) };
        if count > 0 {
            STDIN_LEFTOVER.with(|buffer| {
                buffer
                    .borrow_mut()
                    .extend_from_slice(&chunk[..count as usize])
            });
        } else if count == 0 {
            // EOF: hand back whatever is buffered (an unterminated final line), or empty.
            return Ok(STDIN_LEFTOVER.with(|buffer| std::mem::take(&mut *buffer.borrow_mut())));
        } else {
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::WouldBlock => {
                    reregister_readiness(source, token, Interest::READABLE)?;
                    park_on_readiness(token);
                }
                io::ErrorKind::Interrupted => {}
                _ => return Err(error),
            }
        }
    }
}

/// If the leftover buffer holds a complete line (up to and including a `\n`), remove and
/// return it without the newline (and without a preceding `\r`); otherwise `None`.
fn take_buffered_line() -> Option<Vec<u8>> {
    STDIN_LEFTOVER.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        let newline = buffer.iter().position(|&byte| byte == b'\n')?;
        let mut line: Vec<u8> = buffer.drain(..=newline).collect();
        line.pop(); // drop the '\n'
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        Some(line)
    })
}

/// Put stdin into non-blocking mode (once per thread) so a `read` on an empty pipe returns
/// `WouldBlock` and parks the fiber, rather than blocking the single OS thread.
fn set_stdin_nonblocking() {
    if STDIN_NONBLOCKING.with(Cell::get) {
        return;
    }
    // SAFETY: `fcntl` on a valid descriptor; failure is tolerable (we fall back to blocking).
    unsafe {
        let flags = libc::fcntl(STDIN_FD, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(STDIN_FD, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    STDIN_NONBLOCKING.with(|set| set.set(true));
}
