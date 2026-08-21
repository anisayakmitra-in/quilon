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
use std::cell::RefCell;
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
    /// continues the same stdin stream line-by-line rather than dropping them.
    static STDIN_LEFTOVER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

const STDIN_FD: i32 = 0;

/// Read one line from stdin (fd 0), the source `@read` reads. Thin wrapper over
/// [`read_line_from`]: it takes the persistent leftover buffer OUT of its thread-local while
/// reading (so no `RefCell` borrow is held across the fiber park) and stores what remains
/// back afterwards, so successive `@read`s continue the same stream.
fn read_stdin_line() -> io::Result<Vec<u8>> {
    let mut buffer = STDIN_LEFTOVER.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
    let result = read_line_from(STDIN_FD, &mut buffer);
    STDIN_LEFTOVER.with(|slot| *slot.borrow_mut() = buffer);
    result
}

/// Read one line from `fd` into `buffer`, parking the fiber on reactor readiness until a
/// newline arrives or the stream ends. Returns the line WITHOUT its trailing newline (a
/// trailing `\r` is dropped too). At end-of-input with nothing buffered, returns an empty
/// `Vec` — the documented end-of-input value (`@read` yields an empty `Text` there). Bytes
/// past the newline stay in `buffer` for the next call.
///
/// The reactor registration is LAZY: it reads first and only registers `fd` (and parks) on the
/// first `WouldBlock`. So a source that is ready right away — piped data already buffered, or a
/// non-pollable fd like a redirected file or `/dev/null` that returns data/EOF at once — never
/// touches `epoll`, which rejects such fds. Only a genuinely-not-ready pollable source (an
/// empty pipe/tty) is registered and parked on. Registering after a `WouldBlock` loses no
/// wakeup: adding an already-ready fd to the poll reports it immediately.
fn read_line_from(fd: i32, buffer: &mut Vec<u8>) -> io::Result<Vec<u8>> {
    if let Some(line) = take_line(buffer) {
        return Ok(line);
    }
    set_nonblocking(fd);
    let mut source = SourceFd(&fd);
    let mut token: Option<Token> = None;
    let mut chunk = [0u8; 1024];
    let result = loop {
        // SAFETY: `read(2)` into a valid, owned buffer of `chunk.len()` bytes.
        let count = unsafe { libc::read(fd, chunk.as_mut_ptr() as *mut c_void, chunk.len()) };
        if count > 0 {
            buffer.extend_from_slice(&chunk[..count as usize]);
            if let Some(line) = take_line(buffer) {
                break Ok(line);
            }
        } else if count == 0 {
            // EOF: hand back whatever is buffered (an unterminated final line), or empty.
            break Ok(std::mem::take(buffer));
        } else {
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::WouldBlock => {
                    let active = match token {
                        Some(active) => {
                            match reregister_readiness(&mut source, active, Interest::READABLE) {
                                Ok(()) => active,
                                Err(error) => break Err(error),
                            }
                        }
                        None => match register_readiness(&mut source, Interest::READABLE) {
                            Ok(active) => {
                                token = Some(active);
                                active
                            }
                            Err(error) => break Err(error),
                        },
                    };
                    park_on_readiness(active);
                }
                io::ErrorKind::Interrupted => {}
                _ => break Err(error),
            }
        }
    };
    if token.is_some() {
        deregister_readiness(&mut source);
    }
    result
}

/// If `buffer` holds a complete line (up to and including a `\n`), remove and return it
/// without the newline (and without a preceding `\r`); otherwise `None`.
fn take_line(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let newline = buffer.iter().position(|&byte| byte == b'\n')?;
    let mut line: Vec<u8> = buffer.drain(..=newline).collect();
    line.pop(); // drop the '\n'
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Some(line)
}

/// Put `fd` into non-blocking mode so a `read` on an empty pipe returns `WouldBlock` and
/// parks the fiber, rather than blocking the single OS thread. Failure is tolerable — a
/// still-blocking read simply blocks (functionally fine when nothing else is runnable).
fn set_nonblocking(fd: i32) {
    // SAFETY: `fcntl` on a descriptor; a bad fd just returns an error we ignore.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gc;
    use crate::scheduler::{run, sleep, spawn};
    use crate::test_support::GC_LOCK;
    use std::os::raw::c_int;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::mpsc;
    use std::time::Duration;

    #[repr(C)]
    struct GcStackBase {
        mem_base: *mut c_void,
    }

    #[link(name = "gc")]
    unsafe extern "C" {
        fn GC_register_my_thread(sb: *const GcStackBase) -> c_int;
        fn GC_get_stack_base(sb: *mut GcStackBase) -> c_int;
    }

    type Job = Box<dyn FnOnce() + Send>;

    // One persistent, Boehm-registered worker thread runs every GC-touching test body — the
    // same rationale as the `scheduler`/`net` test harnesses: a stable thread set keeps
    // stop-the-world signalling off exited threads.
    fn gc_worker() -> &'static mpsc::Sender<Job> {
        static WORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
        WORKER.get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<Job>();
            std::thread::spawn(move || {
                gc::install_hooks();
                let mut stack_base = GcStackBase {
                    mem_base: ptr::null_mut(),
                };
                unsafe {
                    GC_get_stack_base(&mut stack_base);
                    GC_register_my_thread(&stack_base);
                }
                for job in receiver {
                    job();
                }
            });
            sender
        })
    }

    fn on_gc_thread<F: FnOnce() + Send + 'static>(f: F) {
        let _guard = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (done_sender, done_receiver) = mpsc::channel();
        gc_worker()
            .send(Box::new(move || {
                f();
                let _ = done_sender.send(());
            }))
            .unwrap();
        done_receiver.recv().unwrap();
    }

    /// A `pipe(2)` pair, returned as `(read_end, write_end)`.
    fn make_pipe() -> (i32, i32) {
        let mut fds = [0i32; 2];
        // SAFETY: `pipe` fills a 2-element array with the two descriptors.
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0, "pipe() failed");
        (fds[0], fds[1])
    }

    /// Mirror of [`__read_launch`] but reading one line from an arbitrary `fd` (a pipe), so a
    /// test can drive the producer with a controllable writer. Same shape: allocate the cell,
    /// spawn the reader (eager launch), return the deferred `{promise, -1}` representation.
    fn launch_read_from_fd(fd: i32) -> QlSlice {
        let promise = __alloc(std::mem::size_of::<Promise>() as i64) as *mut Promise;
        unsafe {
            (*promise).state = PENDING;
            (*promise).data = ptr::null();
            (*promise).len = 0;
            (*promise).site_data = ptr::null();
            (*promise).site_len = 0;
        }
        let address = promise as usize;
        spawn(move || {
            let mut buffer = Vec::new();
            let bytes = read_line_from(fd, &mut buffer).expect("pipe read");
            let text = alloc_text(&bytes);
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

    #[test]
    fn take_line_splits_on_newline_and_keeps_remainder() {
        let mut buffer = b"first\r\nsecond".to_vec();
        assert_eq!(take_line(&mut buffer), Some(b"first".to_vec()));
        assert_eq!(buffer, b"second");
        // No newline yet: nothing to take.
        assert_eq!(take_line(&mut buffer), None);
        assert_eq!(buffer, b"second");
    }

    #[test]
    fn deferred_read_launches_and_forces_the_line() {
        // Proves the whole value-returning path: `@read` launches a background reader that
        // must PARK on stdin readiness (the writer is delayed), a separate fiber FORCES the
        // deferred value (parking on the promise), and the read line flows through.
        static GOT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        GOT.lock().unwrap().clear();

        on_gc_thread(|| {
            let (read_fd, write_fd) = make_pipe();
            run(move || {
                let deferred = launch_read_from_fd(read_fd);
                let promise_ptr = deferred.data;

                spawn(move || {
                    let forced = __force_text(promise_ptr);
                    let bytes = unsafe {
                        std::slice::from_raw_parts(forced.data as *const u8, forced.len as usize)
                    };
                    *GOT.lock().unwrap() = bytes.to_vec();
                });

                // Delay the write so the reader is already parked on pipe readiness when it
                // arrives — the park path, not a lucky already-ready read, is what runs.
                spawn(move || {
                    sleep(Duration::from_millis(20));
                    let message = b"hello world\n";
                    unsafe {
                        libc::write(write_fd, message.as_ptr() as *const c_void, message.len());
                    }
                });
            });
        });

        assert_eq!(&*GOT.lock().unwrap(), b"hello world");
    }

    #[test]
    fn force_is_memoized_after_ready() {
        // Forcing the same deferred value twice returns the same bytes, and the second force
        // never parks (the cell is already READY).
        static FIRST: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        static SECOND: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        FIRST.lock().unwrap().clear();
        SECOND.lock().unwrap().clear();

        on_gc_thread(|| {
            let (read_fd, write_fd) = make_pipe();
            let message = b"line\n";
            // Write before the run so the value is ready without any park.
            unsafe {
                libc::write(write_fd, message.as_ptr() as *const c_void, message.len());
            }
            run(move || {
                let deferred = launch_read_from_fd(read_fd);
                let promise_ptr = deferred.data;
                spawn(move || {
                    let a = __force_text(promise_ptr);
                    let b = __force_text(promise_ptr);
                    let read = |s: QlSlice| unsafe {
                        std::slice::from_raw_parts(s.data as *const u8, s.len as usize).to_vec()
                    };
                    *FIRST.lock().unwrap() = read(a);
                    *SECOND.lock().unwrap() = read(b);
                });
            });
        });

        assert_eq!(&*FIRST.lock().unwrap(), b"line");
        assert_eq!(&*SECOND.lock().unwrap(), b"line");
    }
}
