// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Non-blocking TCP for the fiber scheduler.
//!
//! [`TcpListener`] and [`TcpStream`] wrap `mio`'s non-blocking sockets and register
//! them with the reactor's `Poll`. Every op that would block parks the calling fiber
//! (via [`crate::scheduler::park_on_readiness`]) instead of spinning or blocking the OS
//! thread: it (re)registers the source for the readiness it needs, yields to the
//! scheduler, and is resumed only when the reactor reports that token ready — exactly
//! the way [`crate::scheduler::sleep`] parks on a deadline. Many sockets thus make
//! progress cooperatively on one thread.
//!
//! This tier is internal Rust; no Quilon-visible `@` primitive is wired to it yet.
//!
//! GC note: parking is transparent to the collector. A parked fiber's stack — with
//! its live roots — is scanned by [`crate::gc`]'s `GC_push_other_roots` callback,
//! which pushes every registered fiber that is not currently running, regardless of
//! *why* it is parked. A socket-blocked fiber is therefore covered identically to a
//! sleeping one; `tests::socket_parked_fiber_roots_survive_collection` proves it.

use crate::scheduler::{
    deregister_readiness, park_on_readiness, register_readiness, reregister_readiness,
};
use mio::event::Source;
use mio::{Interest, Token};
use std::io::{self, Read, Write};
use std::net::SocketAddr;

fn would_block(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::WouldBlock
}

/// Retry a non-blocking op on `source`, parking the fiber on each `WouldBlock` until
/// the reactor reports `interest` ready. Reregistering before every park is what
/// makes the edge-triggered poll re-check readiness and lose no wakeup.
fn io_loop<S: Source, T>(
    source: &mut S,
    token: Token,
    interest: Interest,
    mut op: impl FnMut(&mut S) -> io::Result<T>,
) -> io::Result<T> {
    loop {
        match op(source) {
            Ok(value) => return Ok(value),
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(ref e) if would_block(e) => {
                reregister_readiness(source, token, interest)?;
                park_on_readiness(token);
            }
            Err(e) => return Err(e),
        }
    }
}

/// A non-blocking, reactor-registered TCP listener.
pub struct TcpListener {
    inner: mio::net::TcpListener,
    token: mio::Token,
}

impl TcpListener {
    /// Bind and register for read (connection) readiness.
    pub fn bind(addr: SocketAddr) -> io::Result<TcpListener> {
        let mut inner = mio::net::TcpListener::bind(addr)?;
        let token = register_readiness(&mut inner, Interest::READABLE)?;
        Ok(TcpListener { inner, token })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    /// Accept one connection, parking until a client is ready. The accepted stream is
    /// registered with the reactor for later read/write parking.
    pub fn accept(&mut self) -> io::Result<TcpStream> {
        let (mut inner, _peer) = io_loop(&mut self.inner, self.token, Interest::READABLE, |l| {
            l.accept()
        })?;
        let token = register_readiness(&mut inner, Interest::READABLE)?;
        Ok(TcpStream { inner, token })
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        deregister_readiness(&mut self.inner);
    }
}

/// A non-blocking, reactor-registered TCP stream.
pub struct TcpStream {
    inner: mio::net::TcpStream,
    token: mio::Token,
}

impl TcpStream {
    /// Initiate a connection and park until it completes. A non-blocking connect
    /// returns immediately; the socket becomes writable once the handshake finishes,
    /// so we register for write readiness, park, then confirm via `SO_ERROR` /
    /// `peer_addr` (a spurious writable wakeup before completion re-parks, never
    /// spins).
    pub fn connect(addr: SocketAddr) -> io::Result<TcpStream> {
        let mut inner = mio::net::TcpStream::connect(addr)?;
        let token = register_readiness(&mut inner, Interest::WRITABLE)?;
        let mut stream = TcpStream { inner, token };
        loop {
            park_on_readiness(stream.token);
            if let Some(err) = stream.inner.take_error()? {
                return Err(err);
            }
            match stream.inner.peer_addr() {
                Ok(_) => return Ok(stream),
                // Handshake not finished yet: re-arm write interest and park again.
                Err(ref e) if e.kind() == io::ErrorKind::NotConnected || would_block(e) => {
                    reregister_readiness(&mut stream.inner, stream.token, Interest::WRITABLE)?;
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.peer_addr()
    }

    /// Read once, parking until readable. Returns `Ok(0)` at EOF (peer closed);
    /// connection-reset and other errors propagate. Callers wanting a fixed length
    /// loop over this (partial reads are normal for TCP).
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        io_loop(&mut self.inner, self.token, Interest::READABLE, |s| {
            s.read(&mut *buf)
        })
    }

    /// Write once, parking until writable. May write fewer bytes than offered; use
    /// [`write_all`](Self::write_all) to send an entire buffer.
    pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        io_loop(&mut self.inner, self.token, Interest::WRITABLE, |s| {
            s.write(buf)
        })
    }

    /// Write the whole buffer, looping over partial writes and parking on each
    /// `WouldBlock`.
    pub fn write_all(&mut self, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            match self.write(buf) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "wrote zero bytes to socket",
                    ));
                }
                Ok(n) => buf = &buf[n..],
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        deregister_readiness(&mut self.inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gc;
    use crate::mem::__alloc;
    use crate::scheduler::{run, sleep, spawn};
    use crate::test_support::GC_LOCK;
    use std::os::raw::{c_int, c_void};
    use std::ptr;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    #[link(name = "gc")]
    unsafe extern "C" {
        fn GC_gcollect();
        fn GC_register_my_thread(sb: *const GcStackBase) -> c_int;
        fn GC_get_stack_base(sb: *mut GcStackBase) -> c_int;
    }

    #[repr(C)]
    struct GcStackBase {
        mem_base: *mut c_void,
    }

    type Job = Box<dyn FnOnce() + Send>;

    // A single persistent Boehm-registered worker thread runs every GC-touching test
    // body (see the identical rationale in `scheduler`'s tests): funneling fiber work
    // onto one long-lived registered thread keeps Boehm's thread set stable so
    // stop-the-world signalling never targets an exited thread.
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

    fn loopback() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    /// Read exactly `buf.len()` bytes, looping over partial reads; errors on early EOF.
    fn read_exact(stream: &mut TcpStream, buf: &mut [u8]) {
        let mut filled = 0;
        while filled < buf.len() {
            let n = stream.read(&mut buf[filled..]).unwrap();
            assert!(n > 0, "unexpected EOF at {filled}/{}", buf.len());
            filled += n;
        }
    }

    #[test]
    fn echo_round_trip_on_one_thread() {
        static SERVER_DONE: AtomicBool = AtomicBool::new(false);
        static CLIENT_DONE: AtomicBool = AtomicBool::new(false);
        static GOT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        SERVER_DONE.store(false, Ordering::SeqCst);
        CLIENT_DONE.store(false, Ordering::SeqCst);
        GOT.lock().unwrap().clear();

        on_gc_thread(|| {
            run(|| {
                let mut listener = TcpListener::bind(loopback()).unwrap();
                let addr = listener.local_addr().unwrap();

                spawn(move || {
                    let mut conn = listener.accept().unwrap();
                    let mut buf = [0u8; 4];
                    read_exact(&mut conn, &mut buf);
                    // Echo the request straight back.
                    conn.write_all(&buf).unwrap();
                    SERVER_DONE.store(true, Ordering::SeqCst);
                });

                spawn(move || {
                    let mut stream = TcpStream::connect(addr).unwrap();
                    stream.write_all(b"ping").unwrap();
                    let mut buf = [0u8; 4];
                    read_exact(&mut stream, &mut buf);
                    *GOT.lock().unwrap() = buf.to_vec();
                    CLIENT_DONE.store(true, Ordering::SeqCst);
                });
            });
        });

        assert!(SERVER_DONE.load(Ordering::SeqCst), "server fiber finished");
        assert!(CLIENT_DONE.load(Ordering::SeqCst), "client fiber finished");
        assert_eq!(&*GOT.lock().unwrap(), b"ping");
    }

    #[test]
    fn reactor_services_sleep_and_socket_together() {
        // A fiber sleeps while the client/server pair does socket IO: proves one
        // `Poll::poll` services both the timer and socket readiness.
        static SLEPT: AtomicBool = AtomicBool::new(false);
        static ECHOED: AtomicBool = AtomicBool::new(false);
        SLEPT.store(false, Ordering::SeqCst);
        ECHOED.store(false, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                let mut listener = TcpListener::bind(loopback()).unwrap();
                let addr = listener.local_addr().unwrap();

                spawn(|| {
                    sleep(Duration::from_millis(30));
                    SLEPT.store(true, Ordering::SeqCst);
                });

                spawn(move || {
                    let mut conn = listener.accept().unwrap();
                    let mut buf = [0u8; 5];
                    read_exact(&mut conn, &mut buf);
                    conn.write_all(&buf).unwrap();
                });

                spawn(move || {
                    // Delay the connect so the sleeper is already parked on a timer
                    // while this fiber parks on socket readiness.
                    sleep(Duration::from_millis(5));
                    let mut stream = TcpStream::connect(addr).unwrap();
                    stream.write_all(b"hello").unwrap();
                    let mut buf = [0u8; 5];
                    read_exact(&mut stream, &mut buf);
                    assert_eq!(&buf, b"hello");
                    ECHOED.store(true, Ordering::SeqCst);
                });
            });
        });

        assert!(SLEPT.load(Ordering::SeqCst), "sleeping fiber woke");
        assert!(ECHOED.load(Ordering::SeqCst), "socket echo completed");
    }

    #[test]
    fn socket_parked_fiber_roots_survive_collection() {
        // A fiber holds the only references to GC allocations on its own stack, then
        // parks on a socket READ (no data yet). While it is socket-parked, a sibling
        // forces a collection; then it sends data, waking the reader, which verifies
        // its objects are byte-for-byte intact — proving socket-parked stacks are
        // scanned exactly like sleep-parked ones.
        const N: usize = 32;
        const LEN: usize = 96;
        static VERIFIED: AtomicUsize = AtomicUsize::new(0);
        VERIFIED.store(0, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                let mut listener = TcpListener::bind(loopback()).unwrap();
                let addr = listener.local_addr().unwrap();

                spawn(move || {
                    let mut conn = listener.accept().unwrap();
                    let mut held = [ptr::null_mut::<u8>(); N];
                    for (i, slot) in held.iter_mut().enumerate() {
                        let p = __alloc(LEN as i64) as *mut u8;
                        unsafe { ptr::write_bytes(p, (i as u8).wrapping_add(1), LEN) };
                        *slot = p;
                    }
                    let held = std::hint::black_box(held);
                    // Parks here on socket readiness while the client collects.
                    let mut buf = [0u8; 2];
                    read_exact(&mut conn, &mut buf);
                    // Churn the heap to reclaim anything wrongly freed, then verify.
                    for _ in 0..64 {
                        let p = __alloc(LEN as i64) as *mut u8;
                        unsafe { ptr::write_bytes(p, 0xEE, LEN) };
                        std::hint::black_box(p);
                    }
                    let mut ok = 0;
                    for (i, &p) in held.iter().enumerate() {
                        let want = (i as u8).wrapping_add(1);
                        if (0..LEN).all(|k| unsafe { *p.add(k) } == want) {
                            ok += 1;
                        }
                    }
                    VERIFIED.store(ok, Ordering::SeqCst);
                });

                spawn(move || {
                    let mut stream = TcpStream::connect(addr).unwrap();
                    // Let the server accept, allocate, and park on read.
                    sleep(Duration::from_millis(20));
                    unsafe { GC_gcollect() };
                    stream.write_all(b"go").unwrap();
                });
            });
        });

        assert_eq!(VERIFIED.load(Ordering::SeqCst), N);
    }
}
