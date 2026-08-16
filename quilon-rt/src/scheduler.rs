// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! A cooperative, single-threaded fiber scheduler — the bedrock of Quilon's
//! concurrency model (colorless implicit futures). [`spawn`] creates a stackful
//! `corosensei` fiber and enqueues it; [`run`] drives a ready-queue + reactor loop
//! that resumes fibers until each finishes or parks, blocks the [`Reactor`] until
//! the nearest wake deadline, and wakes due fibers. [`sleep`] is the first yield
//! primitive: called from inside a fiber, it parks the fiber with a deadline and
//! yields to the scheduler.
//!
//! This tier is internal Rust today (no Quilon-visible `@` primitive yet); the
//! surface arrives in a later step. The subtle fiber-stack GC scanning lives in
//! [`crate::gc`].

use crate::gc;
use crate::reactor::Reactor;
use corosensei::stack::{DefaultStack, Stack};
use corosensei::{Coroutine, CoroutineResult, Yielder};
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ptr;
use std::time::{Duration, Instant};

/// Per-fiber stack size. corosensei's `DefaultStack` adds a low-end guard page and
/// rounds the mapping up to a page boundary. Keep this a multiple of the page size
/// (512 KiB divides every common page size): then the writable region reaches
/// `base()` exactly, so the GC scan range `[limit + page, base)` has no PROT_NONE
/// gap at the top for `GC_push_all_eager` to fault on.
const FIBER_STACK_SIZE: usize = 512 * 1024;

/// What a fiber yields to the scheduler when it parks.
enum Park {
    /// Park until `Instant`, then become ready again.
    Sleep(Instant),
}

type FiberCoroutine = Coroutine<(), Park, (), DefaultStack>;
type FiberYielder = Yielder<(), Park>;

struct Fiber {
    coroutine: FiberCoroutine,
    /// The fiber's stack base (top of its GC-scannable range); set as Boehm's stack
    /// bottom while this fiber runs. The full range is mirrored in the GC registry.
    stack_high: usize,
}

struct Scheduler {
    /// Slab of live fibers indexed by id; `None` marks a free slot.
    fibers: Vec<Option<Fiber>>,
    free: Vec<usize>,
    ready: VecDeque<usize>,
    /// Parked-on-sleep fibers: `(wake deadline, id)`.
    timers: Vec<(Instant, usize)>,
}

impl Scheduler {
    fn new() -> Self {
        Scheduler {
            fibers: Vec::new(),
            free: Vec::new(),
            ready: VecDeque::new(),
            timers: Vec::new(),
        }
    }

    fn alloc_slot(&mut self, fiber: Fiber) -> usize {
        if let Some(id) = self.free.pop() {
            self.fibers[id] = Some(fiber);
            id
        } else {
            self.fibers.push(Some(fiber));
            self.fibers.len() - 1
        }
    }
}

thread_local! {
    /// The active scheduler for this thread. `spawn`/`sleep` reach it here. Borrowed
    /// only in short scopes on the scheduler's own turns — never held across a
    /// `resume`, so a resumed fiber may re-enter (e.g. call `spawn`) freely.
    static SCHEDULER: RefCell<Option<Scheduler>> = const { RefCell::new(None) };

    /// The running fiber's `Yielder`, so the free-standing `sleep` can suspend
    /// without threading the yielder through every call. Set on fiber entry and
    /// re-set by `sleep` after each resume (other fibers run in between and clobber
    /// this shared cell).
    static CURRENT_YIELDER: Cell<*const FiberYielder> = const { Cell::new(ptr::null()) };
}

/// Run `f` against the active scheduler. A short borrow only — never held across a
/// `resume`, so a resumed fiber may re-enter the scheduler freely.
fn with_scheduler<R>(f: impl FnOnce(&mut Scheduler) -> R) -> R {
    SCHEDULER.with(|s| f(s.borrow_mut().as_mut().expect("no active scheduler")))
}

/// Spawn `f` as a new fiber and enqueue it. Callable before [`run`] (to seed the
/// first fiber) or from within a running fiber (to spawn children). Panics if no
/// scheduler is active.
pub fn spawn<F: FnOnce() + 'static>(f: F) {
    let stack = DefaultStack::new(FIBER_STACK_SIZE).expect("failed to allocate fiber stack");
    let base = stack.base().get();
    let limit = stack.limit().get();
    // Usable region is [limit + guard_page, base); the guard page sits at the low
    // end of the mapping. Scanning from just above it never faults on PROT_NONE.
    let stack_low = limit + page_size();
    let stack_high = base;

    let coroutine: FiberCoroutine = Coroutine::with_stack(stack, move |yielder, ()| {
        CURRENT_YIELDER.with(|c| c.set(yielder as *const FiberYielder));
        f();
    });

    SCHEDULER.with(|s| {
        let mut slot = s.borrow_mut();
        let scheduler = slot
            .as_mut()
            .expect("spawn() called with no active scheduler");
        let id = scheduler.alloc_slot(Fiber {
            coroutine,
            stack_high,
        });
        scheduler.ready.push_back(id);
        gc::register(id, stack_low, stack_high);
    });
}

/// Park the current fiber until `duration` elapses, yielding to the scheduler. Must
/// be called from within a fiber (panics otherwise).
pub fn sleep(duration: Duration) {
    let yielder = CURRENT_YIELDER.get();
    assert!(!yielder.is_null(), "sleep() called outside a fiber");
    let deadline = Instant::now() + duration;
    // SAFETY: `yielder` points at the live `Yielder` for this fiber, valid for the
    // whole fiber body (it is a parameter of the corosensei closure we are inside).
    unsafe { (*yielder).suspend(Park::Sleep(deadline)) };
    // Resumed: sibling fibers ran and overwrote the shared cell; restore ours so
    // later code on this fiber still finds its yielder.
    CURRENT_YIELDER.set(yielder);
}

/// Run the scheduler until every fiber has finished. Seeds `main` as the first
/// fiber, then loops: drain the ready queue (resuming each fiber until it finishes
/// or parks), block the reactor until the nearest wake deadline, and move due
/// fibers back to ready.
///
/// Must be called on a thread registered with the Boehm GC (the process main
/// thread is registered by `GC_init`; tests register explicitly). Not re-entrant.
pub fn run<F: FnOnce() + 'static>(main: F) {
    gc::install_hooks();
    gc::begin_run();

    let already = SCHEDULER.with(|s| s.borrow().is_some());
    assert!(!already, "run() is not re-entrant");
    SCHEDULER.with(|s| *s.borrow_mut() = Some(Scheduler::new()));
    let mut reactor = Reactor::new().expect("failed to create reactor");

    spawn(main);

    loop {
        // Pop the next ready fiber AND move it out of the slab in one borrow, so no
        // SCHEDULER borrow is held across `resume` (the fiber may re-enter the
        // scheduler, e.g. call `spawn`).
        while let Some((id, mut fiber)) = with_scheduler(|scheduler| {
            scheduler
                .ready
                .pop_front()
                .map(|id| (id, scheduler.fibers[id].take().unwrap()))
        }) {
            gc::enter_fiber(id, fiber.stack_high);
            let result = fiber.coroutine.resume(());
            gc::leave_fiber();

            match result {
                CoroutineResult::Yield(Park::Sleep(deadline)) => with_scheduler(|scheduler| {
                    scheduler.fibers[id] = Some(fiber);
                    scheduler.timers.push((deadline, id));
                }),
                CoroutineResult::Return(()) => {
                    // Unregister the stack range before dropping the fiber, which
                    // unmaps its stack: never leave a range in the GC registry that
                    // points at freed memory.
                    gc::unregister(id);
                    drop(fiber);
                    with_scheduler(|scheduler| {
                        scheduler.fibers[id] = None;
                        scheduler.free.push(id);
                    });
                }
            }
        }

        // Ready queue is empty: either everything finished, or fibers are parked.
        let timeout = with_scheduler(|scheduler| {
            scheduler
                .timers
                .iter()
                .map(|(deadline, _)| *deadline)
                .min()
                .map(|next| next.saturating_duration_since(Instant::now()))
        });
        match timeout {
            None => break, // no ready fibers and no timers => all fibers done
            Some(remaining) => reactor.wait(Some(remaining)),
        }

        // Move due timers back to ready.
        with_scheduler(|scheduler| {
            let now = Instant::now();
            let mut i = 0;
            while i < scheduler.timers.len() {
                if scheduler.timers[i].0 <= now {
                    let (_, id) = scheduler.timers.swap_remove(i);
                    scheduler.ready.push_back(id);
                } else {
                    i += 1;
                }
            }
        });
    }

    SCHEDULER.with(|s| *s.borrow_mut() = None);
}

fn page_size() -> usize {
    // SAFETY: sysconf with a valid name has no preconditions.
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if n > 0 { n as usize } else { 4096 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::__alloc;
    use crate::test_support::GC_LOCK;
    use std::os::raw::{c_int, c_void};
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

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

    /// A single, persistent, Boehm-registered worker thread that every GC-touching
    /// test dispatches its body to. Boehm's stop-the-world uses signals to suspend
    /// registered threads; if collections ran on the ephemeral per-test threads the
    /// harness spawns, Boehm would try to signal threads that have since exited and
    /// abort ("Signals delivery fails constantly"). Funneling all fiber work onto one
    /// long-lived registered thread keeps Boehm's thread set stable (main + worker).
    fn gc_worker() -> &'static mpsc::Sender<Job> {
        static WORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
        WORKER.get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<Job>();
            std::thread::spawn(move || {
                // Hooks first (they call GC_allow_register_threads), then register
                // this worker once, for the life of the process.
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

    /// Run `f` on the persistent GC worker, serialized against every other
    /// GC-touching test via `GC_LOCK`, and block until it completes.
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

    fn collect() {
        unsafe { GC_gcollect() };
    }

    /// Fill a fresh GC allocation of `len` bytes with `byte` and return the pointer.
    fn alloc_filled(len: usize, byte: u8) -> *mut u8 {
        let p = __alloc(len as i64) as *mut u8;
        assert!(!p.is_null());
        unsafe { ptr::write_bytes(p, byte, len) };
        p
    }

    fn all_bytes(p: *mut u8, len: usize, byte: u8) -> bool {
        (0..len).all(|i| unsafe { *p.add(i) } == byte)
    }

    #[test]
    fn sleep_wakes_in_deadline_order() {
        static ORDER: Mutex<Vec<u32>> = Mutex::new(Vec::new());
        ORDER.lock().unwrap().clear();

        on_gc_thread(|| {
            run(|| {
                // Spawn out of deadline order; assert they complete in deadline order.
                spawn(|| {
                    sleep(Duration::from_millis(60));
                    ORDER.lock().unwrap().push(60);
                });
                spawn(|| {
                    sleep(Duration::from_millis(20));
                    ORDER.lock().unwrap().push(20);
                });
                spawn(|| {
                    sleep(Duration::from_millis(40));
                    ORDER.lock().unwrap().push(40);
                });
                sleep(Duration::from_millis(10));
                ORDER.lock().unwrap().push(10);
            });
        });

        assert_eq!(*ORDER.lock().unwrap(), vec![10, 20, 40, 60]);
    }

    #[test]
    fn parked_fiber_stack_roots_survive_collection() {
        // Proves (a): a parked fiber's stack roots are pushed by the callback, so a
        // collection driven while it sleeps does not free its data.
        const N: usize = 32;
        const LEN: usize = 96;
        static VERIFIED: AtomicUsize = AtomicUsize::new(0);
        VERIFIED.store(0, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                // Fiber A: allocate, keep the ONLY references on its own stack, sleep
                // long enough for B to force a collection, then verify intact.
                spawn(|| {
                    let mut held = [ptr::null_mut::<u8>(); N];
                    for (i, slot) in held.iter_mut().enumerate() {
                        *slot = alloc_filled(LEN, (i as u8).wrapping_add(1));
                    }
                    let held = std::hint::black_box(held);
                    sleep(Duration::from_millis(40));
                    // Churn the heap after the collection to reuse any wrongly-freed
                    // space, then confirm every held object is byte-for-byte intact.
                    for _ in 0..64 {
                        std::hint::black_box(alloc_filled(LEN, 0xEE));
                    }
                    let mut ok = 0;
                    for (i, &p) in held.iter().enumerate() {
                        if all_bytes(p, LEN, (i as u8).wrapping_add(1)) {
                            ok += 1;
                        }
                    }
                    VERIFIED.store(ok, Ordering::SeqCst);
                });
                // Fiber B: wake first and force a collection while A is parked.
                spawn(|| {
                    sleep(Duration::from_millis(10));
                    collect();
                });
            });
        });

        assert_eq!(VERIFIED.load(Ordering::SeqCst), N);
    }

    #[test]
    fn running_fiber_stack_roots_survive_collection() {
        // Proves (b): a collection triggered while executing ON a fiber stack scans
        // the correct range (via GC_set_stackbottom), so live roots survive.
        const N: usize = 64;
        const LEN: usize = 128;
        static VERIFIED: AtomicUsize = AtomicUsize::new(0);
        VERIFIED.store(0, Ordering::SeqCst);

        on_gc_thread(|| {
            run(|| {
                spawn(|| {
                    let mut held = [ptr::null_mut::<u8>(); N];
                    for (i, slot) in held.iter_mut().enumerate() {
                        *slot = alloc_filled(LEN, (i as u8).wrapping_add(1));
                    }
                    let held = std::hint::black_box(held);
                    // Collect while running on the fiber stack.
                    collect();
                    for _ in 0..128 {
                        std::hint::black_box(alloc_filled(LEN, 0xEE));
                    }
                    let mut ok = 0;
                    for (i, &p) in held.iter().enumerate() {
                        if all_bytes(p, LEN, (i as u8).wrapping_add(1)) {
                            ok += 1;
                        }
                    }
                    VERIFIED.store(ok, Ordering::SeqCst);
                });
            });
        });

        assert_eq!(VERIFIED.load(Ordering::SeqCst), N);
    }
}
